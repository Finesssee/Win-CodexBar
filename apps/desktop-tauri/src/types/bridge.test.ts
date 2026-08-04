import { describe, it, expect } from "vitest";
import type { Language, LocaleStrings, SettingsSnapshot } from "./bridge";

describe("Language type", () => {
  it("accepts supported locale labels as valid union members", () => {
    // Type-level assertion: this assignment must compile (tsc --noEmit gate).
    // Vitest strips types at transform time, so the runtime assertion only
    // exercises value correctness; tsc provides the RED/GREEN gate.
    const lang: Language = "spanish";
    expect(lang).toBe("spanish");
    const langKo: Language = "korean";
    expect(langKo).toBe("korean");
    const langZhTw: Language = "chinesetraditional";
    expect(langZhTw).toBe("chinesetraditional");
  });

  it("allows 'spanish' in LocaleStrings payload", () => {
    const payload: LocaleStrings = {
      language: "spanish",
      entries: { TabGeneral: "General" },
    };
    expect(payload.language).toBe("spanish");
    expect(payload.entries.TabGeneral).toBe("General");

    const payloadKo: LocaleStrings = {
      language: "korean",
      entries: { TabGeneral: "일반" },
    };
    expect(payloadKo.language).toBe("korean");
    expect(payloadKo.entries.TabGeneral).toBe("일반");

    const payloadZhTw: LocaleStrings = {
      language: "chinesetraditional",
      entries: { TabGeneral: "一般" },
    };
    expect(payloadZhTw.language).toBe("chinesetraditional");
    expect(payloadZhTw.entries.TabGeneral).toBe("一般");
  });

  it("allows 'spanish' in SettingsSnapshot.uiLanguage", () => {
    const snap: SettingsSnapshot = {
      enabledProviders: [],
      refreshIntervalSecs: 300,
    adaptiveRefresh: false,
      refreshAllProvidersOnMenuOpen: false,
      startAtLogin: false,
      startMinimized: false,
      showNotifications: true,
      soundEnabled: true,
      notificationSoundTheme: "windows",
      notificationSoundPaths: {
        predictiveWarning: null,
        highUsage: null,
        criticalUsage: null,
        exhausted: null,
        statusIssue: null,
        sessionDepleted: null,
        sessionRestored: null,
      },
      soundVolume: 100,
      highUsageThreshold: 70,
      criticalUsageThreshold: 90,
      predictivePaceWarningEnabled: false,
      trayIconMode: "single",
      switcherShowsIcons: true,
      menuBarShowsHighestUsage: true,
      menuBarShowsPercent: true,
      showAsUsed: false,
      showAllTokenAccountsInMenu: true,
      enableAnimations: true,
      resetTimeRelative: true,
      showResetWhenExhausted: false,
      menuBarDisplayMode: "compact",
      windowScalePercent: 125,
      trayScalePercent: 100,
      powertoysStatusPipeEnabled: false,
      hidePersonalInfo: false,
      autoDownloadUpdates: false,
      installUpdatesOnQuit: false,
      globalShortcut: "",
      codexCustomSessionsDirs: [],
      updateChannel: "stable",
      uiLanguage: "spanish",
      theme: "dark",
      claudeAvoidKeychainPrompts: true,
      codexSparkUsageVisible: true,
      disableKeychainAccess: false,
      providerMetrics: {},
      floatBarEnabled: false,
      floatBarOpacity: 0.9,
      floatBarScale: 100,
      floatBarOrientation: "horizontal",
      floatBarStyle: "floating",
      floatBarClickThrough: false,
      floatBarProviderIds: [],
      floatBarDarkText: false,
      floatBarShowResetInline: false,
      floatBarShowCost: false,
      claudeDailyRoutinesUsageVisible: true,
      alibabaTokenPlanRegion: "cn",
      weeklyProgressWorkDays: null,
    };
    expect(snap.uiLanguage).toBe("spanish");

    const snapKo: SettingsSnapshot = {
      ...snap,
      uiLanguage: "korean",
    };
    expect(snapKo.uiLanguage).toBe("korean");

    const snapZhTw: SettingsSnapshot = {
      ...snap,
      uiLanguage: "chinesetraditional",
    };
    expect(snapZhTw.uiLanguage).toBe("chinesetraditional");
  });
});
