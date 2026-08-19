import { useCallback, useEffect, useState } from "react";
import { useLocale } from "../../../hooks/useLocale";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { playNotificationSound, quitApp } from "../../../lib/tauri";
import { Field, NumberInput, Select, Toggle } from "../../../components/FormControls";
import type {
  Language,
  LanguageOption,
  NotificationSoundEvent,
  NotificationSoundPaths,
  NotificationSoundTheme,
  ThemePreference,
  UsageThresholdOverride,
} from "../../../types/bridge";
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
  { value: "turkish", display: "Türkçe" },
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

const NOTIFICATION_SOUND_THEME_OPTIONS: {
  value: NotificationSoundTheme;
  labelKey: LocaleKey;
}[] = [
  { value: "windows", labelKey: "NotificationSoundThemeWindows" },
  { value: "codexBar", labelKey: "NotificationSoundThemeCodexBar" },
];

type NotificationSoundPathKey = keyof NotificationSoundPaths;

const NOTIFICATION_SOUND_EVENTS: {
  event: NotificationSoundEvent;
  pathKey: NotificationSoundPathKey;
  labelKey: LocaleKey;
  helperKey: LocaleKey;
}[] = [
  {
    event: "predictiveWarning",
    pathKey: "predictiveWarning",
    labelKey: "NotificationSoundEventPredictiveWarning",
    helperKey: "NotificationSoundEventPredictiveWarningHelper",
  },
  {
    event: "highUsage",
    pathKey: "highUsage",
    labelKey: "NotificationSoundEventHighUsage",
    helperKey: "NotificationSoundEventHighUsageHelper",
  },
  {
    event: "criticalUsage",
    pathKey: "criticalUsage",
    labelKey: "NotificationSoundEventCriticalUsage",
    helperKey: "NotificationSoundEventCriticalUsageHelper",
  },
  {
    event: "exhausted",
    pathKey: "exhausted",
    labelKey: "NotificationSoundEventExhausted",
    helperKey: "NotificationSoundEventExhaustedHelper",
  },
  {
    event: "statusIssue",
    pathKey: "statusIssue",
    labelKey: "NotificationSoundEventStatusIssue",
    helperKey: "NotificationSoundEventStatusIssueHelper",
  },
  {
    event: "sessionDepleted",
    pathKey: "sessionDepleted",
    labelKey: "NotificationSoundEventSessionDepleted",
    helperKey: "NotificationSoundEventSessionDepletedHelper",
  },
  {
    event: "sessionRestored",
    pathKey: "sessionRestored",
    labelKey: "NotificationSoundEventSessionRestored",
    helperKey: "NotificationSoundEventSessionRestoredHelper",
  },
];

const NOTIFICATION_SOUND_THEME_SELECT_MIN_WIDTH = 180;
const NOTIFICATION_SOUND_PREVIEW_DURATION_MS = 1500;


const THEME_OPTIONS: { value: ThemePreference; labelKey: LocaleKey }[] = [
  { value: "auto", labelKey: "ThemeAutoOption" },
  { value: "light", labelKey: "ThemeLightOption" },
  { value: "dark", labelKey: "ThemeDarkOption" },
]

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

function blurOnEnter(event: { key: string; currentTarget: { blur: () => void } }) {
  if (event.key === "Enter") event.currentTarget.blur();
}

function isNotificationSoundTheme(v: string): v is NotificationSoundTheme {
  return v === "windows" || v === "codexBar";
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
  const [playingSound, setPlayingSound] = useState<NotificationSoundEvent | null>(null);
  const [soundError, setSoundError] = useState<string | null>(null);
  const [languageOptions, setLanguageOptions] = useState<LanguageOption[]>(
    FALLBACK_LANGUAGE_OPTIONS,
  );

  useEffect(() => {
    invoke<LanguageOption[]>("get_available_languages")
      .then(setLanguageOptions)
      .catch(() => {}); // graceful fallback to static default
  }, []);

  const handleTestSound = useCallback((event: NotificationSoundEvent) => {
    setSoundError(null);
    setPlayingSound(event);
    const timeoutId = window.setTimeout(
      () => setPlayingSound(null),
      NOTIFICATION_SOUND_PREVIEW_DURATION_MS,
    );
    void playNotificationSound(event).catch((error: unknown) => {
      window.clearTimeout(timeoutId);
      setPlayingSound(null);
      setSoundError(error instanceof Error ? error.message : String(error));
    });
  }, []);

  const handleChooseSound = useCallback(
    async (pathKey: NotificationSoundPathKey) => {
      setSoundError(null);
      try {
        const selected = await open({
          multiple: false,
          directory: false,
          filters: [{ name: t("NotificationSoundWaveFile"), extensions: ["wav"] }],
        });
        if (typeof selected === "string") {
          set({
            notificationSoundPaths: {
              ...settings.notificationSoundPaths,
              [pathKey]: selected,
            },
          });
        }
      } catch (error: unknown) {
        setSoundError(error instanceof Error ? error.message : String(error));
      }
    },
    [set, settings.notificationSoundPaths, t],
  );

  const handleClearSound = useCallback(
    (pathKey: NotificationSoundPathKey) => {
      setSoundError(null);
      set({
        notificationSoundPaths: {
          ...settings.notificationSoundPaths,
          [pathKey]: null,
        },
      });
    },
    [set, settings.notificationSoundPaths],
  );

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
        <h3 className="settings-section__title">{t("SectionTheme")}</h3>
        <div className="settings-section__group">
          <Field label={t("ThemeLabel")} description={t("ThemeHelper")}>
            <Select
              value={settings.theme}
              disabled={saving}
              ariaLabel={t("ThemeLabel")}
              options={THEME_OPTIONS.map((option) => ({
                value: option.value,
                label: t(option.labelKey),
              }))}
              onChange={(value) => set({ theme: value as ThemePreference })}
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
            <Toggle
              checked={settings.soundEnabled}
              disabled={saving}
              onChange={(v) => set({ soundEnabled: v })}
            />
          </Field>
          {settings.soundEnabled && (
            <>
              <Field
                label={t("NotificationSoundTheme")}
                description={t("NotificationSoundThemeHelper")}
              >
                <Select
                  value={settings.notificationSoundTheme}
                  disabled={saving}
                  ariaLabel={t("NotificationSoundTheme")}
                  minWidth={NOTIFICATION_SOUND_THEME_SELECT_MIN_WIDTH}
                  options={NOTIFICATION_SOUND_THEME_OPTIONS.map((option) => ({
                    value: option.value,
                    label: t(option.labelKey),
                  }))}
                  onChange={(value) => {
                    if (isNotificationSoundTheme(value)) {
                      set({ notificationSoundTheme: value });
                    }
                  }}
                />
              </Field>
              {NOTIFICATION_SOUND_EVENTS.map((sound) => {
                const path = settings.notificationSoundPaths[sound.pathKey];
                const label = t(sound.labelKey);
                return (
                  <Field
                    key={sound.event}
                    label={label}
                    description={t(sound.helperKey)}
                  >
                    <div className="notification-sound-row">
                      <button
                        type="button"
                        className="shortcut-capture__button shortcut-capture__button--ghost notification-sound-file"
                        aria-label={`${label}: ${path ? `${fileName(path)}, ` : ""}${t("NotificationSoundChooseFile")}`}
                        title={path ?? t("NotificationSoundUsesTheme")}
                        disabled={saving}
                        onClick={() => void handleChooseSound(sound.pathKey)}
                      >
                        {path ? fileName(path) : t("NotificationSoundChooseFile")}
                      </button>
                      <button
                        type="button"
                        className="shortcut-capture__button shortcut-capture__button--ghost"
                        aria-label={`${label}: ${t("NotificationTestSound")}`}
                        disabled={saving || playingSound !== null}
                        onClick={() => handleTestSound(sound.event)}
                      >
                        {playingSound === sound.event
                          ? t("NotificationTestSoundPlaying")
                          : t("NotificationTestSound")}
                      </button>
                      {path && (
                        <button
                          type="button"
                          className="shortcut-capture__button shortcut-capture__button--ghost"
                          aria-label={`${label}: ${t("NotificationSoundClearFile")}`}
                          disabled={saving}
                          onClick={() => handleClearSound(sound.pathKey)}
                        >
                          {t("NotificationSoundClearFile")}
                        </button>
                      )}
                    </div>
                  </Field>
                );
              })}
              {soundError && (
                <p className="settings-section__error" role="alert">
                  {soundError}
                </p>
              )}
            </>
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
          <Field
            label={t("LowPowerMode")}
            description={t("LowPowerModeHelper")}
          >
            <Select
              value={settings.lowPowerModePreference ?? (settings.lowPowerMode ? "on" : "off")}
              disabled={saving}
              options={[
                { value: "off", label: t("LowPowerModeOff") },
                { value: "on", label: t("LowPowerModeOn") },
                { value: "automatic", label: t("LowPowerModeAutomatic") },
              ]}
              onChange={(v) => set({
                lowPowerModePreference: v as "off" | "on" | "automatic",
              })}
            />
          </Field>
          <div style={{ display: "flex", justifyContent: "flex-end", paddingTop: 8 }}>
            <button
              type="button"
              className="credential-btn credential-btn--primary"
              onClick={() => void quitApp()}
            >
              {t("MenuQuit")}
            </button>
          </div>
        </div>
      </section>}
    </>
  );
}
