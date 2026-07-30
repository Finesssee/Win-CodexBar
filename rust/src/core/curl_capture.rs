//! Shared parsing for DevTools "Copy as cURL" captures pasted into manual-auth
//! provider settings. Ported from upstream `CurlCaptureParser` (v0.46.0).

use regex_lite::Regex;
use reqwest::Url;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Extracts the request URL from the standard DevTools "Copy as cURL" shape,
/// where the URL is the first argument after `curl`.
///
/// Returns `None` for malformed captures or option-first forms
/// (e.g. `curl --location 'https://…'`).
pub fn request_url(raw: &str) -> Option<String> {
    // (?s)(?:^|\s)curl\s+(?:\$'…'|'…'|"…"|bare)
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r#"(?s)(?:^|\s)curl(?:\.exe)?\s+(?:\$'((?:\\.|[^'])*)'|'([^']*)'|"((?:\\.|[^"])*)"|([^\s\\]+))"#,
        )
        .expect("curl url regex")
    });
    let caps = re.captures(raw)?;
    let value = if let Some(m) = caps.get(1) {
        unescape_shell_segment(m.as_str(), true)
    } else if let Some(m) = caps.get(2) {
        m.as_str().to_string()
    } else if let Some(m) = caps.get(3) {
        unescape_shell_segment(m.as_str(), false)
    } else {
        let m = caps.get(4)?;
        unescape_shell_segment(m.as_str(), false)
    };
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    // Minimal URL sanity: scheme + host present.
    let parsed = Url::parse(value).ok()?;
    if parsed.scheme().is_empty() || parsed.host_str().is_none() {
        return None;
    }
    Some(value.to_string())
}

/// Splits a raw cURL capture into the raw `-H "Name: Value"` / `--header '…'`
/// header field strings it contains, unescaping shell quoting (including
/// ANSI-C `$'...'` strings).
pub fn header_fields(raw: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // regex_lite has no lookaround; allow space, `=`, or glued quote (`-H'…'`).
        Regex::new(
            r#"(?s)(?:^|\s)(?:-H|--header)(?:\s*=\s*|\s+)?(?:\$'((?:\\.|[^'])*)'|'([^']*)'|"((?:\\.|[^"])*)"|(\S+))"#,
        )
        .expect("curl header regex")
    });
    let mut fields = Vec::new();
    for caps in re.captures_iter(raw) {
        if let Some(m) = caps.get(1) {
            fields.push(unescape_shell_segment(m.as_str(), true));
        } else if let Some(m) = caps.get(2) {
            fields.push(m.as_str().to_string());
        } else if let Some(m) = caps.get(3) {
            fields.push(unescape_shell_segment(m.as_str(), false));
        } else if let Some(m) = caps.get(4) {
            fields.push(unescape_shell_segment(m.as_str(), false));
        }
    }
    fields
}

/// Returns the value of the first header field whose name matches `name`
/// (case-insensitive).
pub fn header_value(name: &str, fields: &[String]) -> Option<String> {
    for field in fields {
        let (raw_name, value) = split_header_field(field)?;
        if raw_name.eq_ignore_ascii_case(name) && !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

/// Builds a `[canonicalHeaderName: value]` map from `fields`, keeping only
/// headers whose lowercased name appears in `allowlist` (mapping lowercase
/// name → canonical HTTP header name).
pub fn forwarded_headers(
    fields: &[String],
    allowlist: &HashMap<&str, &str>,
) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    for field in fields {
        let Some((raw_name, value)) = split_header_field(field) else {
            continue;
        };
        if raw_name.is_empty() || value.is_empty() {
            continue;
        }
        let key = raw_name.to_ascii_lowercase();
        if let Some(&canonical) = allowlist.get(key.as_str()) {
            headers.insert(canonical.to_string(), value.to_string());
        }
    }
    headers
}

/// Convenience: allowlist as a slice of `(lowercase, Canonical)` pairs.
pub fn forwarded_headers_from_pairs(
    fields: &[String],
    allowlist: &[(&str, &str)],
) -> HashMap<String, String> {
    let map: HashMap<&str, &str> = allowlist.iter().copied().collect();
    forwarded_headers(fields, &map)
}

fn split_header_field(field: &str) -> Option<(&str, &str)> {
    let colon = field.find(':')?;
    let name = field[..colon].trim();
    let value = field[colon + 1..].trim();
    Some((name, value))
}

/// Unescape a shell-quoted segment. When `ansi` is true (ANSI-C `$'…'`),
/// `\n`/`\r`/`\t` expand; otherwise only backslash-escapes of the next char
/// (and line-continuation `\\\n`) are handled.
pub fn unescape_shell_segment(raw: &str, ansi: bool) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        let Some(next) = chars.next() else {
            break;
        };
        match next {
            'n' if ansi => output.push('\n'),
            'r' if ansi => output.push('\r'),
            't' if ansi => output.push('\t'),
            '\n' => {}
            other => output.push(other),
        }
    }
    output
}

/// True when `raw` looks like a DevTools cURL capture rather than a bare cookie header.
pub fn looks_like_curl_capture(raw: &str) -> bool {
    let lower = raw.trim_start().to_ascii_lowercase();
    lower.starts_with("curl ") || lower.starts_with("curl.exe ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_request_url_from_standard_devtools_capture() {
        let curl = "curl 'https://example.com/api/usage' -H 'Authorization: Bearer fake-token'";
        assert_eq!(
            request_url(curl).as_deref(),
            Some("https://example.com/api/usage")
        );
    }

    #[test]
    fn extracts_request_url_from_double_quoted_and_bare_captures() {
        assert_eq!(
            request_url("curl \"https://example.com/double\"")
                .as_deref()
                .and_then(|u| Url::parse(u).ok())
                .map(|u| u.path().to_string())
                .as_deref(),
            Some("/double")
        );
        assert_eq!(
            request_url("curl https://example.com/bare")
                .as_deref()
                .and_then(|u| Url::parse(u).ok())
                .map(|u| u.path().to_string())
                .as_deref(),
            Some("/bare")
        );
    }

    #[test]
    fn request_url_returns_none_for_option_first_or_malformed() {
        assert_eq!(request_url("curl --location 'https://example.com'"), None);
        assert_eq!(request_url("not-curl 'https://example.com'"), None);
    }

    #[test]
    fn extracts_header_fields_from_double_quoted_headers() {
        let curl = r#"curl 'https://example.com' --header "Authorization: Bearer fake-token" --header "Cookie: session=abc""#;
        let fields = header_fields(curl);
        assert!(
            fields
                .iter()
                .any(|f| f == "Authorization: Bearer fake-token")
        );
        assert!(fields.iter().any(|f| f == "Cookie: session=abc"));
    }

    #[test]
    fn extracts_header_fields_from_single_quoted_h_flags() {
        let curl = "curl 'https://example.com' -H 'Authorization: Bearer fake-token'";
        assert_eq!(
            header_fields(curl),
            vec!["Authorization: Bearer fake-token"]
        );
    }

    #[test]
    fn header_value_lookup_is_case_insensitive() {
        let fields = vec!["AUTHORIZATION: Bearer fake-token".to_string()];
        assert_eq!(
            header_value("authorization", &fields).as_deref(),
            Some("Bearer fake-token")
        );
    }

    #[test]
    fn header_value_lookup_returns_none_for_missing() {
        let fields = vec!["Cookie: session=abc".to_string()];
        assert_eq!(header_value("Authorization", &fields), None);
    }

    #[test]
    fn forwarded_headers_respects_allowlist() {
        let fields = vec![
            "Authorization: Bearer fake-token".to_string(),
            "Cookie: session=abc".to_string(),
            "X-Not-Allowed: nope".to_string(),
        ];
        let headers = forwarded_headers_from_pairs(
            &fields,
            &[("authorization", "Authorization"), ("cookie", "Cookie")],
        );
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer fake-token")
        );
        assert_eq!(
            headers.get("Cookie").map(String::as_str),
            Some("session=abc")
        );
        assert!(!headers.contains_key("X-Not-Allowed"));
    }

    #[test]
    fn forwarded_headers_can_include_authorization() {
        let fields = vec!["authorization: Bearer fake-token".to_string()];
        let headers = forwarded_headers_from_pairs(&fields, &[("authorization", "Authorization")]);
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer fake-token")
        );
    }

    #[test]
    fn unescapes_ansi_c_quoted_segments() {
        assert_eq!(unescape_shell_segment(r"a\nb\tc", true), "a\nb\tc");
        assert_eq!(
            unescape_shell_segment(r#"say \"hi\""#, false),
            r#"say "hi""#
        );
    }
}
