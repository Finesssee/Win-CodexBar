import { useEffect, useState } from "react";
import type { LocaleKey } from "../../../../../i18n/keys";
import { getSettingsSnapshot, updateSettings } from "../../../../../lib/tauri";

interface Props {
  t: (key: LocaleKey) => string;
}

/**
 * Claude-specific credential/options.
 *
 * Port of the `ProviderId::Claude` branch of the "Options" block in
 * `rust/src/native_ui/preferences.rs::render_provider_detail_panel`.
 * Exposes "Avoid keychain prompts" and "Show Daily Routines usage".
 * The broader `disable_keychain_access` master switch lives in Advanced.
 */
export function ClaudeCreds({ t }: Props) {
  const [avoidKeychain, setAvoidKeychain] = useState<boolean | null>(null);
  const [showDailyRoutines, setShowDailyRoutines] = useState<boolean | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    getSettingsSnapshot()
      .then((s) => {
        if (cancelled) return;
        setAvoidKeychain(s.claudeAvoidKeychainPrompts);
        setShowDailyRoutines(s.claudeDailyRoutinesUsageVisible ?? true);
      })
      .catch((e) => !cancelled && setError(String(e)));
    return () => {
      cancelled = true;
    };
  }, []);

  const toggleAvoidKeychain = async (next: boolean) => {
    setSaving(true);
    try {
      const updated = await updateSettings({
        claudeAvoidKeychainPrompts: next,
      });
      setAvoidKeychain(updated.claudeAvoidKeychainPrompts);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const toggleDailyRoutines = async (next: boolean) => {
    setSaving(true);
    try {
      const updated = await updateSettings({
        claudeDailyRoutinesUsageVisible: next,
      });
      setShowDailyRoutines(updated.claudeDailyRoutinesUsageVisible ?? next);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  if (avoidKeychain === null || showDailyRoutines === null) return null;

  return (
    <section className="provider-detail-section">
      <h4>{t("CredentialsSectionTitle")}</h4>
      <label className="provider-detail-toggle">
        <input
          type="checkbox"
          checked={avoidKeychain}
          disabled={saving}
          onChange={(e) => void toggleAvoidKeychain(e.target.checked)}
        />
        <span>
          <span className="provider-detail-toggle__label">
            {t("ProviderClaudeAvoidKeychainPrompts")}
          </span>
          <span className="provider-detail-toggle__helper">
            {t("ProviderClaudeAvoidKeychainPromptsHelp")}
          </span>
        </span>
      </label>
      <label className="provider-detail-toggle">
        <input
          type="checkbox"
          checked={showDailyRoutines}
          disabled={saving}
          onChange={(e) => void toggleDailyRoutines(e.target.checked)}
        />
        <span>
          <span className="provider-detail-toggle__label">
            {t("ProviderClaudeDailyRoutinesUsage")}
          </span>
          <span className="provider-detail-toggle__helper">
            {t("ProviderClaudeDailyRoutinesUsageHelp")}
          </span>
        </span>
      </label>
      {error && <div className="provider-detail-error">{error}</div>}
    </section>
  );
}
