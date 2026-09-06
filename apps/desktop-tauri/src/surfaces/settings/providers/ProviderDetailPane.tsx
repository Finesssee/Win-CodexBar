import { useCallback, useEffect, useReducer } from "react";
import type { SettingsSnapshot, SettingsUpdate } from "../../../types/bridge";
import { useLocale } from "../../../hooks/useLocale";
import {
  getCredentialStorageStatus,
  getProviderCookieSourceOptions,
  getProviderDetail,
  getProviderRegionOptions,
  getTokenAccountProviders,
  openProviderDashboard,
  openProviderStatusPage,
  refreshProviders,
  revokeProviderCredentials,
  setProviderGatewayUrl,
  triggerProviderLogin,
} from "../../../lib/tauri";
import { listen } from "@tauri-apps/api/event";

import {
  initialState,
  providerDetailPaneReducer,
} from "./providerDetailPaneState";
import { buildSubtitle } from "./providerDetailFormat";
import { IdentitySection } from "./sections/IdentitySection";
import { UsageSection } from "./sections/UsageSection";
import { PaceSection } from "./sections/PaceSection";
import { CostSection } from "./sections/CostSection";
import { QuickActionsSection } from "./sections/QuickActionsSection";
import { ChartsSection } from "./sections/charts/ChartsSection";
import { CookieSourceSection } from "./sections/CookieSourceSection";
import { UsageSourceSection } from "./sections/UsageSourceSection";
import { RegionSection } from "./sections/RegionSection";
import { CodexUsageOptions } from "./sections/credentials/CodexUsageOptions";
import { CodexAccountsSection } from "./sections/credentials/CodexAccountsSection";
import { TokenAccountsPanel } from "../tokens/TokenAccountsPanel";
import { ApiKeySection } from "./ApiKeySection";
import { CookieSection } from "./CookieSection";
import { MenuBarMetricSection } from "./sections/MenuBarMetricSection";
import { AccentColorSection } from "./sections/AccentColorSection";
import { ProviderIssueNotice } from "./sections/ProviderIssueNotice";
import { CredentialStorageSection } from "./sections/CredentialStorageSection";
import { CredentialsDispatcher } from "./sections/CredentialsDispatcher";
import { WayfinderGatewaySection } from "./sections/WayfinderGatewaySection";

interface Props {
  providerId: string | null;
  cookieDomain?: string | null;
  resetTimeRelative: boolean;
  providerMetrics: SettingsSnapshot["providerMetrics"];
  /** Per-provider accent color overrides (CLI name → hex color). */
  providerAccentColors: SettingsSnapshot["providerAccentColors"];
  wayfinderGatewayUrl: string;
  settingsDisabled: boolean;
  onSettingsChange: (patch: SettingsUpdate) => void;
}

/**
 * Orchestrates the Settings → Providers right-hand detail pane.
 *
 * Top-level port of
 * `rust/src/native_ui/preferences.rs::render_provider_detail_panel`
 * (lines 4301–6698).
 */
export function ProviderDetailPane({
  providerId,
  cookieDomain = null,
  resetTimeRelative,
  providerMetrics,
  providerAccentColors,
  wayfinderGatewayUrl,
  settingsDisabled,
  onSettingsChange,
}: Props) {
  const { t } = useLocale();
  const [state, dispatch] = useReducer(
    providerDetailPaneReducer,
    { wayfinderGatewayUrl, providerId },
    (init) => initialState(init.wayfinderGatewayUrl, init.providerId),
  );

  if (
    providerId !== state.syncedProviderId ||
    wayfinderGatewayUrl !== state.syncedGatewayUrl
  ) {
    dispatch({ type: "SYNC_PROPS", providerId, wayfinderGatewayUrl });
  }

  const {
    detail,
    cookieOptions,
    regionOptions,
    credentialStatus,
    credentialRevision,
    tokenProviderIds,
    loading,
    error,
    busy,
    gatewayDraft,
    gatewayError,
  } = state;

  const setErr = (e: unknown) =>
    dispatch({ type: "SET_ERROR", error: String(e) });
  const setBusy = (value: boolean) =>
    dispatch({ type: "SET_BUSY", busy: value });

  const load = useCallback(async (id: string, signal?: { stale: boolean }) => {
    dispatch({ type: "LOAD_START" });
    try {
      const [next, cookieOpts, regionOpts, storageStatus] = await Promise.all([
        getProviderDetail(id),
        getProviderCookieSourceOptions(id),
        getProviderRegionOptions(id),
        getCredentialStorageStatus(),
      ]);
      if (signal?.stale) return;
      dispatch({
        type: "LOAD_SUCCESS",
        detail: next,
        cookieOptions: cookieOpts,
        regionOptions: regionOpts,
        credentialStatus: storageStatus,
      });
    } catch (e) {
      if (signal?.stale) return;
      dispatch({ type: "LOAD_ERROR", error: String(e) });
    } finally {
      // Keep stale guard — concurrent provider switches must not clear loading
      // for the in-flight replacement load.
      if (!signal?.stale) dispatch({ type: "LOAD_FINISH" });
    }
  }, []);

  const saveGateway = async () => {
    dispatch({ type: "SAVE_GATEWAY_START" });
    try {
      await setProviderGatewayUrl("wayfinder", gatewayDraft);
      await load("wayfinder");
    } catch (e) {
      dispatch({ type: "SAVE_GATEWAY_ERROR", error: String(e) });
    } finally {
      setBusy(false);
    }
  };

  // Load the set of providers that support token accounts once.
  useEffect(() => {
    let cancelled = false;
    void getTokenAccountProviders()
      .then((list) => {
        if (!cancelled) {
          dispatch({
            type: "SET_TOKEN_PROVIDERS",
            ids: list.map((p) => p.providerId),
          });
        }
      })
      .catch(() => {
        // Non-fatal: inline token section will simply not render.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (!providerId) {
      dispatch({ type: "LOAD_CLEAR", mode: "empty" });
      return;
    }
    // Clear stale detail immediately so we don't render the old provider
    dispatch({ type: "LOAD_CLEAR", mode: "switch" });
    const signal = { stale: false };
    void load(providerId, signal);
    return () => {
      signal.stale = true;
    };
  }, [providerId, load]);

  // Live-refresh when a new snapshot lands for this provider.
  useEffect(() => {
    if (!providerId) return;
    const signal = { stale: false };
    const unlistenPromise = listen<{ providerId?: string }>(
      "provider-updated",
      (event) => {
        const pid = event.payload?.providerId;
        if (!pid || pid === providerId) void load(providerId, signal);
      },
    );
    return () => {
      signal.stale = true;
      void unlistenPromise.then((fn) => fn());
    };
  }, [providerId, load]);

  if (!providerId) {
    return emptyDetail(t("StateNoProviderSelected"));
  }
  if (loading && !detail) {
    return emptyDetail(t("StateLoadingProviders"));
  }
  if (error && !detail) {
    return emptyDetail(`${t("StateError")}: ${error}`, true);
  }
  if (!detail) return null;

  const subtitle = buildSubtitle(detail, t);
  const reload = () => void load(detail.id);
  const credKey = `${detail.id}-${credentialRevision}`;

  const handleRefresh = async () => {
    setBusy(true);
    try {
      await refreshProviders();
    } catch (e) {
      setErr(e);
    } finally {
      setBusy(false);
    }
  };

  const handleSwitchAccount = async () => {
    setBusy(true);
    try {
      await triggerProviderLogin(detail.id);
      dispatch({ type: "BUMP_CREDENTIAL_REVISION" });
      await refreshProviders();
      await load(detail.id);
    } catch (e) {
      setErr(e);
    } finally {
      setBusy(false);
    }
  };

  const handleRevokeCredentials = async () => {
    setBusy(true);
    dispatch({ type: "SET_ERROR", error: null });
    try {
      await revokeProviderCredentials(detail.id);
      dispatch({ type: "BUMP_CREDENTIAL_REVISION" });
      await load(detail.id);
    } catch (e) {
      setErr(e);
    } finally {
      setBusy(false);
    }
  };

  const handleOpenDashboard = () => {
    void openProviderDashboard(detail.id).catch(setErr);
  };
  const handleOpenStatusPage = () => {
    void openProviderStatusPage(detail.id).catch(setErr);
  };
  const handleBuyCredits = () => {
    if (detail.buyCreditsUrl) {
      void openProviderDashboard(detail.id).catch(setErr);
    }
  };

  return (
    <div className="provider-detail">
      <IdentitySection provider={detail} subtitle={subtitle} t={t} />

      {detail.lastError && (
        <ProviderIssueNotice detail={detail} t={t} />
      )}

      <UsageSection
        provider={detail}
        resetTimeRelative={resetTimeRelative}
        t={t}
      />
      {detail.id === "wayfinder" && (
        <WayfinderGatewaySection
          draft={gatewayDraft}
          error={gatewayError}
          busy={busy}
          disabled={settingsDisabled}
          onDraftChange={(draft) =>
            dispatch({ type: "SET_GATEWAY_DRAFT", draft })
          }
          onSave={() => void saveGateway()}
          t={t}
        />
      )}
      <MenuBarMetricSection
        provider={detail}
        providerMetrics={providerMetrics}
        disabled={settingsDisabled}
        t={t}
        onChange={onSettingsChange}
      />
      <AccentColorSection
        providerId={detail.id}
        accentColor={providerAccentColors[detail.id] ?? null}
        t={t}
        onChange={onSettingsChange}
      />
      <PaceSection pace={detail.pace} t={t} />
      <CostSection cost={detail.cost} t={t} />

      <UsageSourceSection
        providerId={detail.id}
        currentValue={detail.usageSource}
        t={t}
        onChanged={reload}
      />
      {!(detail.id === "alibabatokenplan" && detail.usageSource === "cli") && (
        <CookieSourceSection
          providerId={detail.id}
          currentValue={detail.cookieSource}
          options={cookieOptions}
          t={t}
          onChanged={reload}
        />
      )}
      <RegionSection
        providerId={detail.id}
        currentValue={detail.region}
        options={regionOptions}
        t={t}
        onChanged={reload}
      />
      <CredentialsDispatcher providerId={detail.id} t={t} />
      {detail.id === "codex" && <CodexUsageOptions t={t} />}
      {detail.id === "codex" && <CodexAccountsSection t={t} />}
      <CredentialStorageSection
        status={credentialStatus}
        busy={busy}
        onRevoke={handleRevokeCredentials}
        t={t}
      />
      {tokenProviderIds.has(detail.id) && (
        <TokenAccountsPanel
          key={`token-${credKey}`}
          providerId={detail.id}
          compact
        />
      )}
      <ApiKeySection key={`api-${credKey}`} providerId={detail.id} />
      <CookieSection
        key={`cookie-${credKey}`}
        providerId={detail.id}
        cookieDomain={cookieDomain}
      />
      <ChartsSection
        providerId={detail.id}
        accountEmail={detail.email}
        accentColor={providerAccentColors[detail.id]}
        t={t}
      />

      <QuickActionsSection
        provider={detail}
        busy={busy}
        onRefresh={handleRefresh}
        onSwitchAccount={handleSwitchAccount}
        onOpenDashboard={handleOpenDashboard}
        onOpenStatusPage={handleOpenStatusPage}
        onBuyCredits={handleBuyCredits}
        t={t}
      />
    </div>
  );
}

function emptyDetail(message: string, isError = false) {
  return (
    <div className="provider-detail">
      <div
        className={
          isError
            ? "provider-detail-empty provider-detail-empty--error"
            : "provider-detail-empty"
        }
      >
        {message}
      </div>
    </div>
  );
}
