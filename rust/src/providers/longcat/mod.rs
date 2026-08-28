//! LongCat cookie-based quota + fuel-pack provider (upstream 0.44 #1697).
//!
//! Default-disabled. Auth via manual cookie or browser import for longcat.chat.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde_json::Value;

use crate::core::{
    FetchContext, Provider, ProviderError, ProviderFetchResult, ProviderId, ProviderMetadata,
    RateWindow, SourceMode, UsageSnapshot,
};

const HOST: &str = "https://longcat.chat";
const USER_CURRENT: &str = "/api/v1/user-current";
const TOKEN_USAGE: &str = "/api/lc-platform/v1/tokenUsage";
const PENDING_FUEL: &str = "/api/lc-platform/v1/pending-fuel-packages";
/// Live token-pack lot summary (upstream 0.49.4 #2670).
const TOKEN_PACKS_SUMMARY: &str = "/api/pay/quota/metering/token-packs/summary";

pub struct LongCatProvider {
    metadata: ProviderMetadata,
    client: Client,
}

impl LongCatProvider {
    pub fn new() -> Self {
        Self {
            metadata: ProviderMetadata {
                id: ProviderId::LongCat,
                display_name: "LongCat",
                session_label: "Quota",
                weekly_label: "Fuel Pack",
                supports_opus: false,
                supports_credits: false,
                default_enabled: false,
                is_primary: false,
                dashboard_url: Some("https://longcat.chat/platform/"),
                status_page_url: None,
            },
            // Isolated cookie-free client — auth is only the explicit Cookie header.
            client: crate::core::credentialed_http_client_builder()
                .cookie_store(false)
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    async fn get_json(&self, path: &str, cookie: &str) -> Result<Value, ProviderError> {
        let url = format!("{HOST}{path}");
        let resp = self
            .client
            .get(&url)
            .header("Cookie", cookie)
            .header("Accept", "application/json")
            .send()
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::AuthRequired);
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "LongCat API {path} returned HTTP {status}"
            )));
        }
        resp.json()
            .await
            .map_err(|e| ProviderError::Parse(format!("Failed to parse LongCat {path}: {e}")))
    }

    /// POST variant of [`Self::get_json`] used by the token-packs summary
    /// endpoint (upstream 0.49.4 #2670 posts an empty JSON body).
    async fn post_json(&self, path: &str, cookie: &str) -> Result<Value, ProviderError> {
        let url = format!("{HOST}{path}");
        let resp = self
            .client
            .post(&url)
            .header("Cookie", cookie)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .body("{}")
            .send()
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ProviderError::AuthRequired);
        }
        if !status.is_success() {
            return Err(ProviderError::Other(format!(
                "LongCat API {path} returned HTTP {status}"
            )));
        }
        resp.json()
            .await
            .map_err(|e| ProviderError::Parse(format!("Failed to parse LongCat {path}: {e}")))
    }
}

impl Default for LongCatProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for LongCatProvider {
    fn id(&self) -> ProviderId {
        ProviderId::LongCat
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    async fn fetch_usage(&self, ctx: &FetchContext) -> Result<ProviderFetchResult, ProviderError> {
        match ctx.source_mode {
            SourceMode::Auto | SourceMode::Web => {
                let cookie = match ctx.manual_cookie_header.as_deref() {
                    Some(c) => normalize_cookie_header(c).ok_or(ProviderError::NoCookies)?,
                    None => crate::providers::browser_cookie_header(&["longcat.chat"])?,
                };
                let account = self.get_json(USER_CURRENT, &cookie).await?;
                // Meituan-style envelope may return HTTP 200 with business 401.
                if let Some(code) = envelope_code(&account)
                    && (code == 401 || code == 403)
                {
                    return Err(ProviderError::AuthRequired);
                }
                // Upstream 0.49.4 #2670: prefer the token-packs summary lot;
                // only fall back to the legacy token-usage endpoint when no
                // active lot is available.
                let token_packs = match self.post_json(TOKEN_PACKS_SUMMARY, &cookie).await {
                    Ok(v) => Some(v),
                    Err(ProviderError::AuthRequired) => return Err(ProviderError::AuthRequired),
                    Err(err) => {
                        tracing::debug!("LongCat token-packs summary probe failed: {err}");
                        None
                    }
                };
                let usage_raw = if token_packs.as_ref().is_some_and(has_active_token_pack_lot) {
                    None
                } else {
                    Some(self.get_json(TOKEN_USAGE, &cookie).await?)
                };
                let fuel = match self.get_json(PENDING_FUEL, &cookie).await {
                    Ok(v) => Some(v),
                    Err(ProviderError::AuthRequired) => return Err(ProviderError::AuthRequired),
                    Err(_) => None,
                };
                let snap = build_snapshot(
                    &account,
                    token_packs.as_ref(),
                    usage_raw.as_ref(),
                    fuel.as_ref(),
                )?;
                Ok(ProviderFetchResult::new(snap, "web"))
            }
            SourceMode::Cli | SourceMode::OAuth => {
                Err(ProviderError::UnsupportedSource(ctx.source_mode))
            }
        }
    }

    fn available_sources(&self) -> Vec<SourceMode> {
        vec![SourceMode::Auto, SourceMode::Web]
    }
}

fn normalize_cookie_header(raw: &str) -> Option<String> {
    let mut header = raw.trim().to_string();
    if header
        .get(.."cookie:".len())
        .is_some_and(|p| p.eq_ignore_ascii_case("cookie:"))
    {
        header = header["cookie:".len()..].trim().to_string();
    }
    (!header.is_empty()).then_some(header)
}

fn envelope_code(value: &Value) -> Option<i64> {
    value
        .get("code")
        .and_then(|c| c.as_i64())
        .or_else(|| value.get("status").and_then(|c| c.as_i64()))
}

fn envelope_data(value: &Value) -> &Value {
    value.get("data").unwrap_or(value)
}

fn json_f64(value: &Value, key: &str) -> Option<f64> {
    let v = value.get(key)?;
    v.as_f64()
        .or_else(|| v.as_i64().map(|i| i as f64))
        .or_else(|| v.as_str()?.parse().ok())
}

fn json_str(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Whether the token-packs summary carries an ACTIVE lot with a positive
/// total (upstream `activeTokenPackLot`).
fn has_active_token_pack_lot(summary: &Value) -> bool {
    active_token_pack_lot(summary).is_some()
}

fn active_token_pack_lot(summary: &Value) -> Option<Value> {
    let lot = envelope_data(summary).get("currentLot")?;
    if json_str(lot, "status")?.to_uppercase() != "ACTIVE" {
        return None;
    }
    json_f64(lot, "totalToken").filter(|total| *total > 0.0)?;
    Some(lot.clone())
}

fn build_snapshot(
    account: &Value,
    token_packs: Option<&Value>,
    usage_raw: Option<&Value>,
    fuel_raw: Option<&Value>,
) -> Result<UsageSnapshot, ProviderError> {
    let account_data = envelope_data(account);

    let (total, used) = if let Some(lot) = token_packs.and_then(active_token_pack_lot) {
        // Active token-pack lot: consumed/total drive the quota directly.
        let total = json_f64(&lot, "totalToken")
            .ok_or_else(|| ProviderError::Parse("token-packs lot missing totalToken".into()))?;
        let used = json_f64(&lot, "consumedToken").unwrap_or(0.0);
        (total, used)
    } else if let Some(usage_raw) = usage_raw {
        // Legacy token quota: data.usage is the canonical aggregate.
        let usage_outer = envelope_data(usage_raw);
        let usage = usage_outer
            .get("usage")
            .filter(|u| u.is_object())
            .unwrap_or(usage_outer);
        let total = json_f64(usage, "totalToken")
            .ok_or_else(|| ProviderError::Parse("tokenUsage data was missing totalToken".into()))?;
        let remaining = json_f64(usage, "availableToken");
        let used = remaining.map(|r| (total - r).max(0.0)).unwrap_or(0.0);
        (total, used)
    } else {
        return Err(ProviderError::Parse(
            "LongCat usage data was missing (no active token-pack lot and no tokenUsage payload)"
                .into(),
        ));
    };

    let primary = if total > 0.0 {
        let mut w = RateWindow::new(((used / total) * 100.0).clamp(0.0, 100.0));
        // Display-only rendering of token counts; values beyond i64 are
        // unrealistic quota sizes and would only affect this label.
        #[allow(
            clippy::cast_possible_truncation,
            reason = "display-only quota label; token counts beyond i64 are unrealistic"
        )]
        let desc = format!("{}/{}", used as i64, total as i64);
        w.reset_description = Some(desc);
        w
    } else {
        RateWindow::informational("No token quota")
    };

    let account_name = json_str(account_data, "name")
        .or_else(|| json_str(account_data, "nickname"))
        .or_else(|| json_str(account_data, "userName"));

    let mut snap = UsageSnapshot::new(primary);
    if let Some(name) = account_name {
        snap.account_organization = Some(name);
    }

    if let Some(fuel_raw) = fuel_raw {
        let fuel_data = envelope_data(fuel_raw);
        if let Some((total_fuel, remaining_fuel, expiry)) = parse_fuel(fuel_data)
            && total_fuel > 0.0
        {
            let used_fuel = (total_fuel - remaining_fuel).max(0.0);
            let mut secondary =
                RateWindow::new(((used_fuel / total_fuel) * 100.0).clamp(0.0, 100.0));
            secondary.resets_at = expiry;
            // Same display-only label for fuel-pack counts.
            #[allow(
                clippy::cast_possible_truncation,
                reason = "display-only fuel label; fuel counts beyond i64 are unrealistic"
            )]
            let fuel_desc = format!("Fuel pack: {}/{}", remaining_fuel as i64, total_fuel as i64);
            secondary.reset_description = Some(fuel_desc);
            snap = snap.with_secondary(secondary);
        }
    }

    Ok(snap)
}

fn parse_fuel(fuel: &Value) -> Option<(f64, f64, Option<DateTime<Utc>>)> {
    // Accept either { packages: [...] } or a bare array.
    let packages = fuel
        .get("packages")
        .and_then(|p| p.as_array())
        .or_else(|| fuel.as_array())?;

    let mut total = 0.0;
    let mut remaining = 0.0;
    let mut saw_remaining = false;
    let mut nearest: Option<DateTime<Utc>> = None;

    for pkg in packages {
        if let Some(t) = json_f64(pkg, "totalToken")
            .or_else(|| json_f64(pkg, "total"))
            .or_else(|| json_f64(pkg, "amount"))
        {
            total += t.max(0.0);
        }
        if let Some(r) = json_f64(pkg, "availableToken")
            .or_else(|| json_f64(pkg, "remainingToken"))
            .or_else(|| json_f64(pkg, "remaining"))
        {
            remaining += r.max(0.0);
            saw_remaining = true;
        }
        if let Some(raw) = json_str(pkg, "expireTime")
            .or_else(|| json_str(pkg, "expireAt"))
            .or_else(|| json_str(pkg, "expiresAt"))
            && let Ok(dt) = DateTime::parse_from_rfc3339(&raw)
        {
            let utc = dt.with_timezone(&Utc);
            nearest = Some(match nearest {
                Some(n) if n < utc => n,
                _ => utc,
            });
        }
    }

    if total <= 0.0 && !saw_remaining {
        return None;
    }
    if total <= 0.0 {
        total = remaining;
    }
    let remaining = if saw_remaining { remaining } else { total };
    Some((total, remaining, nearest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_quota_and_fuel() {
        let account = json!({ "code": 0, "data": { "name": "cat" } });
        let usage = json!({
            "code": 0,
            "data": {
                "usage": { "totalToken": 1000, "availableToken": 250 }
            }
        });
        let fuel = json!({
            "code": 0,
            "data": {
                "packages": [
                    { "totalToken": 200, "availableToken": 50, "expireTime": "2026-08-01T00:00:00Z" }
                ]
            }
        });
        let snap = build_snapshot(&account, None, Some(&usage), Some(&fuel)).unwrap();
        assert!((snap.primary.used_percent - 75.0).abs() < 0.01);
        assert_eq!(snap.account_organization.as_deref(), Some("cat"));
        let fuel_w = snap.secondary.unwrap();
        assert!((fuel_w.used_percent - 75.0).abs() < 0.01);
    }

    #[test]
    fn active_token_pack_lot_wins_over_legacy_usage() {
        // Upstream 0.49.4 #2670: the token-packs summary lot is the live
        // usage source; the legacy endpoint is only a fallback.
        let account = json!({ "code": 0, "data": { "name": "cat" } });
        let summary = json!({
            "code": 0,
            "data": {
                "currentLot": {
                    "status": "active",
                    "totalToken": 5000,
                    "consumedToken": 1250
                }
            }
        });
        let snap = build_snapshot(&account, Some(&summary), None, None).unwrap();
        assert!((snap.primary.used_percent - 25.0).abs() < 0.01);
        assert_eq!(snap.primary.reset_description.as_deref(), Some("1250/5000"));
    }

    #[test]
    fn inactive_or_empty_lot_falls_back_to_legacy_usage() {
        let account = json!({ "code": 0, "data": { "name": "cat" } });
        let inactive = json!({
            "code": 0,
            "data": {
                "currentLot": { "status": "EXPIRED", "totalToken": 5000, "consumedToken": 10 }
            }
        });
        let usage = json!({
            "code": 0,
            "data": { "usage": { "totalToken": 1000, "availableToken": 900 } }
        });
        let snap = build_snapshot(&account, Some(&inactive), Some(&usage), None).unwrap();
        assert!((snap.primary.used_percent - 10.0).abs() < 0.01);

        let no_total = json!({ "code": 0, "data": { "currentLot": { "status": "ACTIVE" } } });
        assert!(!has_active_token_pack_lot(&no_total));
        assert!(has_active_token_pack_lot(&json!({
            "code": 0,
            "data": { "currentLot": { "status": "ACTIVE", "totalToken": 5 } }
        })));
    }

    #[test]
    fn normalizes_cookie() {
        assert_eq!(
            normalize_cookie_header("Cookie: a=1; b=2").as_deref(),
            Some("a=1; b=2")
        );
    }
}
