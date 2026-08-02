import { useCallback, useEffect, useState } from "react";
import { useLocale } from "../../../hooks/useLocale";
import { invoke } from "@tauri-apps/api/core";
import { playNotificationSound } from "../../../lib/tauri";
import { Field, NumberInput, Select, Toggle } from "../../../components/FormControls";
import type { Language, LanguageOption, UsageThresholdOverride } from "../../../types/bridge";
import type { LocaleKey } from "../../../i18n/keys";
import type { TabProps } from "../settingsTabs";

const FALLBACK_LANGUAGE_OPTIONS: LanguageOption[] = [
  { value: "english", display: "English" },
  { value: "chinese", display: "中文" },
  { value: "chinesetraditional", display: "繁體中文" },
  { value: "japanese", display: "日本語" },
  { value: "korean", display: "한국어" },
  { value: "spanish", display: "Español" },
  { value: "russian", display: "Русский" },
];

const REFRESH_CADENCE_OPTIONS: { value: string; labelKey: LocaleKey }[] = [
  { value: "0", labelKey: "RefreshIntervalManual" },
  { value: "adaptive", labelKey: "RefreshIntervalAdaptive" },
  { value: "60", labelKey: "RefreshInterval1Min" },
  { value: "300", labelKey: "RefreshInterval5Min" },
  { value: "900", labelKey: "RefreshInterval15Min" },
  { value: "1800", labelKey: "RefreshInterval30Min" },
  { value: "3600", labelKey: "RefreshInterval1Hour" },
];

function blurOnEnter(event: { key: string; currentTarget: { blur: () => void } }) {
  if (event.key === "Enter") event.currentTarget.blur();
}

function ThresholdOverrideInputs({
  label,
  value,
  inheritedHigh,
  inheritedCritical,
  highLabel,
  criticalLabel,
  disabled,
  onChange,
}: {
  label: string;
  value: UsageThresholdOverride;
  inheritedHigh: number;
  inheritedCritical: number;
  highLabel: string;
  criticalLabel: string;
  disabled: boolean;
  onChange: (value: UsageThresholdOverride) => void;
}) {
  const [high, setHigh] = useState(() => value.high?.toString() ?? "");
  const [critical, setCritical] = useState(() => value.critical?.toString() ?? "");
  useEffect(() => setHigh(value.high?.toString() ?? ""), [value.high]);
  useEffect(() => setCritical(value.critical?.toString() ?? ""), [value.critical]);
  const commit = () =>
    onChange({
      high: high === "" ? undefined : Math.min(100, Math.max(0, Number(high))),
      critical:
        critical === "" ? undefined : Math.min(100, Math.max(0, Number(critical))),
    });
  return (
    <Field label={label}>
      <div className="settings-inline-fields">
        <input
          type="number"
          value={high}
          min={0}
          max={100}
          disabled={disabled}
          placeholder={String(inheritedHigh)}
          aria-label={`${label} ${highLabel}`}
          onChange={(event) => setHigh(event.target.value)}
          onBlur={commit}
          onKeyDown={blurOnEnter}
        />
        <input
          type="number"
          value={critical}
          min={0}
          max={100}
          disabled={disabled}
          placeholder={String(inheritedCritical)}
          aria-label={`${label} ${criticalLabel}`}
          onChange={(event) => setCritical(event.target.value)}
          onBlur={commit}
          onKeyDown={blurOnEnter}
        />
      </div>
    </Field>
  );
}

export default function GeneralTab({
  mode = "general",
  settings,
  set,
  saving,
}: TabProps & { mode?: "general" | "notifications" }) {
  const { t } = useLocale();
  const [playingSound, setPlayingSound] = useState(false);
  const [languageOptions, setLanguageOptions] = useState<LanguageOption[]>(
    FALLBACK_LANGUAGE_OPTIONS,
  );

  useEffect(() => {
    invoke<LanguageOption[]>("get_available_languages")
      .then(setLanguageOptions)
      .catch(() => {}); // graceful fallback to static default
  }, []);

  const handleTestSound = useCallback(() => {
    setPlayingSound(true);
    void playNotificationSound().catch(() => {});
    window.setTimeout(() => setPlayingSound(false), 1500);
  }, []);

  return (
    <>
      {mode === "general" && <section className="settings-section">
        <h3 className="settings-section__title">{t("SectionLanguage")}</h3>
        <div className="settings-section__group">
          <Field label={t("InterfaceLanguage")}>
            <Select
              value={settings.uiLanguage}
              disabled={saving}
              options={languageOptions.map((opt) => ({
                value: opt.value,
                label: opt.display,
              }))}
              onChange={(v) => set({ uiLanguage: v as Language })}
            />
          </Field>
        </div>
      </section>}

      {mode === "general" && <section className="settings-section">
        <h3 className="settings-section__title">{t("StartupSettings")}</h3>
        <div className="settings-section__group">
          <Field label={t("StartAtLogin")} description={t("StartAtLoginHelper")} leading>
            <Toggle
              checked={settings.startAtLogin}
              disabled={saving}
              onChange={(v) => set({ startAtLogin: v })}
            />
          </Field>
          <Field
            label={t("StartMinimized")}
            description={t("StartMinimizedHelper")}
            leading
          >
            <Toggle
              checked={settings.startMinimized}
              disabled={saving}
              onChange={(v) => set({ startMinimized: v })}
            />
          </Field>
        </div>
      </section>}

      {mode === "notifications" && <section className="settings-section">
        <h3 className="settings-section__title">
          {t("SectionNotifications")}
        </h3>
        <div className="settings-section__group">
          <Field
            label={t("ShowNotifications")}
            description={t("ShowNotificationsHelper")}
            leading
          >
            <Toggle
              checked={settings.showNotifications}
              disabled={saving}
              onChange={(v) => set({ showNotifications: v })}
            />
          </Field>
          <Field
            label={t("PredictivePaceWarnings")}
            description={t("PredictivePaceWarningsHelper")}
            leading
          >
            <Toggle
              checked={settings.predictivePaceWarningEnabled}
              ariaLabel={t("PredictivePaceWarnings")}
              disabled={saving}
              onChange={(v) => set({ predictivePaceWarningEnabled: v })}
            />
          </Field>
          <Field label={t("SoundEnabled")} description={t("SoundEnabledHelper")} leading>
            <div className="sound-enabled-row">
              <Toggle
                checked={settings.soundEnabled}
                disabled={saving}
                onChange={(v) => set({ soundEnabled: v })}
              />
              <button
                type="button"
                className="shortcut-capture__button shortcut-capture__button--ghost"
                disabled={saving || !settings.soundEnabled || playingSound}
                onClick={handleTestSound}
              >
                {playingSound
                  ? t("NotificationTestSoundPlaying")
                  : t("NotificationTestSound")}
              </button>
            </div>
          </Field>
          {settings.soundEnabled && (
            <Field label={t("SoundVolume")} description={t("SoundVolumeHelper")}>
              <NumberInput
                value={settings.soundVolume}
                min={0}
                max={100}
                step={5}
                disabled={saving}
                onChange={(v) => set({ soundVolume: v })}
              />
            </Field>
          )}
        </div>
        <div className="settings-section__group">
          {(["codex", "claude"] as const).flatMap((provider) =>
            (["provider", "session", "weekly"] as const).map((window) => {
              const key = window === "provider" ? provider : `${provider}:${window}`;
              const values = settings.providerUsageThresholds ?? {};
              const providerLabel =
                provider === "codex"
                  ? t("ProviderNameCodex")
                  : t("ProviderNameClaude");
              return (
                <ThresholdOverrideInputs
                  key={key}
                  label={
                    window === "provider"
                      ? providerLabel
                      : `${providerLabel} · ${t(window === "session" ? "ProviderSession" : "ProviderWeekly")}`
                  }
                  value={values[key] ?? {}}
                  inheritedHigh={
                    window === "provider"
                      ? settings.highUsageThreshold
                      : values[provider]?.high ?? settings.highUsageThreshold
                  }
                  inheritedCritical={
                    window === "provider"
                      ? settings.criticalUsageThreshold
                      : values[provider]?.critical ?? settings.criticalUsageThreshold
                  }
                  highLabel={t("HighUsageAlert")}
                  criticalLabel={t("CriticalUsageAlert")}
                  disabled={saving}
                  onChange={(value) => {
                    const next = { ...values };
                    if (value.high === undefined && value.critical === undefined) {
                      set({
                        providerUsageThresholds: Object.fromEntries(
                          Object.entries(next).filter(([entry]) => entry !== key),
                        ),
                      });
                    } else {
                      next[key] = value;
                      set({ providerUsageThresholds: next });
                    }
                  }}
                />
              );
            }),
          )}
        </div>
      </section>}

      {mode === "notifications" && <section className="settings-section">
        <h3 className="settings-section__title">
          {t("SectionUsageThresholds")}
        </h3>
        <div className="settings-section__group">
          <Field
            label={t("HighUsageAlert")}
            description={t("HighUsageWarningHelper")}
          >
            <NumberInput
              value={settings.highUsageThreshold}
              min={0}
              max={100}
              step={5}
              disabled={saving}
              onChange={(v) => set({ highUsageThreshold: v })}
            />
          </Field>
          <Field
            label={t("CriticalUsageAlert")}
            description={t("CriticalUsageWarningHelper")}
          >
            <NumberInput
              value={settings.criticalUsageThreshold}
              min={0}
              max={100}
              step={5}
              disabled={saving}
              onChange={(v) => set({ criticalUsageThreshold: v })}
            />
          </Field>
        </div>
      </section>}

      {/* ── Automation ───────────────────────────────────────────── */}
      {mode === "general" && <section className="settings-section">
        <h3 className="settings-section__title">{t("SectionRefresh")}</h3>
        <div className="settings-section__group">
          <Field
            label={t("RefreshIntervalLabel")}
            description={t("RefreshIntervalHelper")}
          >
            <Select
              value={
                settings.adaptiveRefresh
                  ? "adaptive"
                  : String(settings.refreshIntervalSecs)
              }
              disabled={saving}
              options={REFRESH_CADENCE_OPTIONS.map((o) => ({
                value: o.value,
                label: t(o.labelKey),
              }))}
              onChange={(v) => {
                if (v === "adaptive") {
                  set({ adaptiveRefresh: true });
                  return;
                }
                set({
                  adaptiveRefresh: false,
                  refreshIntervalSecs: Number(v),
                });
              }}
            />
          </Field>
          <Field
            label={t("RefreshAllProvidersOnMenuOpen")}
            description={t("RefreshAllProvidersOnMenuOpenHelper")}
            leading
          >
            <Toggle
              checked={settings.refreshAllProvidersOnMenuOpen}
              disabled={saving}
              onChange={(v) => set({ refreshAllProvidersOnMenuOpen: v })}
            />
          </Field>
        </div>
      </section>}
    </>
  );
}
