import { useEffect, useState } from "react";
import type { LocaleKey } from "../../../../../i18n/keys";
import {
  getProviderWorkspaceId,
  setProviderWorkspaceId,
} from "../../../../../lib/tauri";

interface Props {
  providerId?: string;
  t: (key: LocaleKey) => string;
}

/**
 * OpenAI/Codex-specific detail help.
 *
 * Port of the help strings below the `ProviderId::Codex` toggles in
 * `rust/src/native_ui/preferences.rs::render_provider_detail_panel` (~6625).
 * The toggles themselves (`codex_historical_tracking`,
 * `codex_openai_web_extras`) are not yet persisted through
 * `update_settings` in the Tauri bridge, so this component shows the
 * upstream hint copy only. The toggles will be surfaced once they join
 * the SettingsUpdate bridge (tracked alongside Phase 6e token-accounts).
 */
export function OpenAiExtras({ providerId = "codex", t }: Props) {
  const [projectId, setProjectId] = useState("");
  const [savedProjectId, setSavedProjectId] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const config = extraConfig(providerId, t);

  useEffect(() => {
    if (!WORKSPACE_EXTRA_IDS[providerId]) return;
    let cancelled = false;
    void getProviderWorkspaceId(providerId)
      .then((value) => {
        if (!cancelled) {
          setProjectId(value ?? "");
          setSavedProjectId(value ?? "");
        }
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [providerId]);

  const saveProjectId = async () => {
    setBusy(true);
    setError(null);
    try {
      const next = projectId.trim();
      await setProviderWorkspaceId(providerId, next);
      setSavedProjectId(next);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (config) {
    return (
      <section className="provider-detail-section">
        <h4>{config.title}</h4>
        <label className="provider-detail-field">
          <span className="provider-detail-field__label">
            {config.label}
          </span>
          <input
            className="provider-detail-field__input"
            value={projectId}
            placeholder={config.placeholder}
            spellCheck={false}
            onChange={(event) => setProjectId(event.target.value)}
          />
        </label>
        <div className="provider-detail-helper">
          {config.help}
        </div>
        <div className="provider-detail-actions">
          <button
            type="button"
            className="credential-btn credential-btn--primary"
            disabled={busy || projectId.trim() === savedProjectId}
            onClick={saveProjectId}
          >
            {t("Save")}
          </button>
        </div>
        {error && <div className="provider-detail-error">{error}</div>}
      </section>
    );
  }

  return (
    <section className="provider-detail-section">
      <h4>{t("CredentialsSectionTitle")}</h4>
      <div className="provider-detail-helper">
        {t("ProviderCodexHistoryHelp")}
      </div>
      <div className="provider-detail-helper">
        {t("CredsOpenAiHistoryHelp")}
      </div>
    </section>
  );
}

const WORKSPACE_EXTRA_IDS: Record<string, true> = {
  openaiapi: true,
  litellm: true,
  devin: true,
  opencodego: true,
  zed: true,
  sub2api: true,
  xai: true,
};

function extraConfig(providerId: string, t: Props["t"]) {
  switch (providerId) {
    case "openaiapi":
      return {
        title: t("OpenAiAdminApiTitle"),
        label: t("OpenAiProjectIdLabel"),
        placeholder: t("OpenAiProjectIdPlaceholder"),
        help: t("OpenAiProjectIdHelp"),
      };
    case "litellm":
      return {
        title: t("LiteLlmApiTitle"),
        label: t("LiteLlmBaseUrlLabel"),
        placeholder: t("LiteLlmBaseUrlPlaceholder"),
        help: t("LiteLlmBaseUrlHelp"),
      };
    case "devin":
      return {
        title: t("DevinApiTitle"),
        label: t("DevinOrganizationLabel"),
        placeholder: t("DevinOrganizationPlaceholder"),
        help: t("DevinOrganizationHelp"),
      };
    case "opencodego":
      return {
        title: t("OpenCodeGoWorkspaceTitle"),
        label: t("OpenCodeGoWorkspaceLabel"),
        placeholder: "wrk_...",
        help: t("OpenCodeGoWorkspaceHelp"),
      };
    case "zed":
      return {
        title: t("ZedApiTitle"),
        label: t("ZedApiUrlLabel"),
        placeholder: t("ZedApiUrlPlaceholder"),
        help: t("ZedApiUrlHelp"),
      };
    case "sub2api":
      return {
        title: t("Sub2ApiTitle"),
        label: t("Sub2ApiBaseUrlLabel"),
        placeholder: t("Sub2ApiBaseUrlPlaceholder"),
        help: t("Sub2ApiBaseUrlHelp"),
      };
    case "xai":
      return {
        title: "xAI team",
        label: "Team ID",
        placeholder: "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx",
        help: "Required. Shown in the xAI Console URL and team settings. Or set XAI_TEAM_ID. Pair with a Management API key (not an inference key).",
      };
    default:
      return null;
  }
}
