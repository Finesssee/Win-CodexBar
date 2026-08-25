import { act, render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  getCachedProviders: vi.fn(),
  getProviderChartData: vi.fn(),
  getProviderLocalUsageSummary: vi.fn(),
  refreshProviders: vi.fn(),
  refreshProvidersIfStale: vi.fn(),
  getSettingsSnapshot: vi.fn(),
  updateSettings: vi.fn(),
  getLocaleStrings: vi.fn(),
  setUiLanguage: vi.fn(),
}));

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(),
}));

const windowMocks = vi.hoisted(() => ({
  getCurrentWindow: vi.fn(() => ({
    startDragging: vi.fn().mockResolvedValue(undefined),
  })),
}));

const coreMocks = vi.hoisted(() => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../lib/tauri", () => tauriMocks);
vi.mock("@tauri-apps/api/event", () => eventMocks);
vi.mock("@tauri-apps/api/window", () => windowMocks);
vi.mock("@tauri-apps/api/core", () => coreMocks);

import FloatBar from "./FloatBar";
import { LocaleProvider } from "../i18n/LocaleProvider";
import { buildBundle } from "../test/localeHarness";
import type {
  BootstrapState,
  ProviderUsageSnapshot,
  RateWindowSnapshot,
  SettingsSnapshot,
} from "../types/bridge";

type RateWindowOptions = {
  exhausted?: boolean;
  informational?: boolean;
  resetsAt?: string | null;
  resetDescription?: string | null;
};

function rateWindow(
  used: number,
  opts: RateWindowOptions = {},
): RateWindowSnapshot {
  return {
    usedPercent: used,
    remainingPercent: 100 - used,
    windowMinutes: null,
    resetsAt: opts.resetsAt ?? null,
    resetDescription: opts.resetDescription ?? null,
    isExhausted: opts.exhausted ?? false,
    isInformational: opts.informational,
    reservePercent: null,
    reserveDescription: null,
  };
}

function snapshot(
  id: string,
  display: string,
  used: number,
  opts: {
    exhausted?: boolean;
    error?: string | null;
    resetsAt?: string | null;
    resetDescription?: string | null;
    informational?: boolean;
    secondary?: {
      used: number;
      exhausted?: boolean;
      informational?: boolean;
      resetsAt?: string | null;
      resetDescription?: string | null;
    };
    selected?: {
      used: number;
      exhausted?: boolean;
      informational?: boolean;
      resetsAt?: string | null;
      resetDescription?: string | null;
    };
  } = {},
): ProviderUsageSnapshot {
  const primary = rateWindow(used, opts);
  const secondary = opts.secondary
    ? rateWindow(opts.secondary.used, opts.secondary)
    : null;
  const selectedMetric = opts.selected
    ? rateWindow(opts.selected.used, opts.selected)
    : primary.isInformational && secondary && !secondary.isInformational
      ? secondary
      : primary;

  return {
    providerId: id,
    displayName: display,
    primary,
    selectedMetric,
    secondary,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    cost: null,
    planName: null,
    accountEmail: null,
    sourceLabel: "auto",
    updatedAt: "2026-05-15T00:00:00Z",
    error: opts.error ?? null,
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
  };
}

function settings(overrides: Partial<SettingsSnapshot> = {}): SettingsSnapshot {
  return {
    enabledProviders: ["claude", "codex"],
    refreshIntervalSecs: 300,
    adaptiveRefresh: false,
    refreshAllProvidersOnMenuOpen: false,
    lowPowerMode: false,
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
    highUsageThreshold: 70,
    criticalUsageThreshold: 90,
    predictivePaceWarningEnabled: false,
    trayIconMode: "single",
    switcherShowsIcons: true,
    menuBarShowsHighestUsage: false,
    menuBarShowsPercent: false,
    showAsUsed: true,
    showAllTokenAccountsInMenu: false,
    enableAnimations: true,
    resetTimeRelative: true,
    showResetWhenExhausted: false,
    menuBarDisplayMode: "detailed",
    hidePersonalInfo: false,
    updateChannel: "stable",
    autoDownloadUpdates: false,
    installUpdatesOnQuit: false,
    globalShortcut: "Ctrl+Shift+U",
    codexCustomSessionsDirs: [],
    uiLanguage: "english",
    theme: "dark",
    windowScalePercent: 125,
    trayScalePercent: 100,
    powertoysStatusPipeEnabled: false,
    claudeAvoidKeychainPrompts: false,
    codexSparkUsageVisible: true,
    disableKeychainAccess: false,
    providerMetrics: {},
    floatBarEnabled: true,
    floatBarOpacity: 80,
    floatBarScale: 100,
    floatBarOrientation: "horizontal",
    floatBarStyle: "floating",
    floatBarClickThrough: false,
    floatBarProviderIds: [],
    floatBarDarkText: false,
    floatBarShowResetInline: false,
    floatBarShowCost: false,
    claudeDailyRoutinesUsageVisible: true,
    claudeAllowReadingClaudeCodeCredentials: false,
    alibabaTokenPlanRegion: "cn",
    weeklyProgressWorkDays: null,
    costSummaryDisplayStyle: "compact",
    providerAccentColors: {},
    ...overrides,
  };
}

function bootstrap(settingsOverrides: Partial<SettingsSnapshot> = {}): BootstrapState {
  return {
    contractVersion: "v1",
    providers: [],
    settings: settings(settingsOverrides),
  };
}

function renderFloatBar(state: BootstrapState) {
  return render(
    <LocaleProvider>
      <FloatBar state={state} />
    </LocaleProvider>,
  );
}

describe("FloatBar", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.refreshProviders.mockResolvedValue(undefined);
    tauriMocks.refreshProvidersIfStale.mockResolvedValue(undefined);
    tauriMocks.getProviderLocalUsageSummary.mockResolvedValue(null);
    tauriMocks.getLocaleStrings.mockResolvedValue(
      buildBundle({
        ResetsInHoursMinutes: "Resets in {}h {}m",
        ResetsInDaysHours: "Resets in {}d {}h",
        TrayResetsDueNow: "Resetting",
        PanelToday: "Today",
        PanelUsedSuffix: "used",
        OverviewSpendEstimate: "Estimate",
        ProviderIssueAuthRequired: "Sign-in required",
        ProviderIssueSessionExpired: "Session expired",
        ProviderIssueLegacyTelemetry: "Legacy telemetry unavailable",
        ProviderIssueLocalRuntimeOffline: "Local runtime offline",
        ProviderIssueUnknown: "Usage unavailable",
        FloatBarThirtyDayShort: "30d",
        FloatBarNoProviders: "No providers",
        FloatBarRemainingSuffix: "remaining",
      }),
    );
    eventMocks.listen.mockResolvedValue(() => {});
  });

  it("renders a pill per enabled provider, sorted by usage descending", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("claude", "Claude", 20),
      snapshot("codex", "Codex", 75),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(
      settings({ floatBarShowCost: true }),
    );

    const { container } = renderFloatBar(bootstrap());
    await waitFor(() => {
      const pills = container.querySelectorAll(".floatbar__pill");
      expect(pills.length).toBe(2);
    });

    const titles = Array.from(container.querySelectorAll(".floatbar__pill")).map(
      (el) => el.getAttribute("title") ?? "",
    );
    // Highest used (codex, 75%) shows first; display follows showAsUsed.
    expect(titles[0]).toMatch(/Codex: 75% used/);
    expect(titles[1]).toMatch(/Claude: 20% used/);
  });

  it("uses the selected session window when a weekly window is available", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("claude", "Claude", 20, { secondary: { used: 90 } }),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(
      settings({ providerMetrics: { claude: "session" } }),
    );

    const { container } = renderFloatBar(
      bootstrap({ providerMetrics: { claude: "session" } }),
    );
    await waitFor(() => {
      expect(container.querySelector(".floatbar__pill")?.getAttribute("title")).toContain(
        "Claude: 20% used",
      );
    });
  });

  it("uses the selected weekly window in the floating bar", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("codex", "Codex", 0, {
        informational: true,
        secondary: { used: 37 },
        selected: { used: 37 },
      }),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(
      settings({ providerMetrics: { codex: "weekly" } }),
    );

    const { container } = renderFloatBar(
      bootstrap({ providerMetrics: { codex: "weekly" } }),
    );
    await waitFor(() => {
      expect(container.querySelector(".floatbar__pill")?.getAttribute("title")).toContain(
        "Codex: 37% used",
      );
    });
  });

  it("uses a real secondary window when the primary window is informational", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("claude", "Claude", 10, {
        informational: true,
        secondary: {
          used: 80,
          resetsAt: null,
          resetDescription: "Resets in 2 hours",
        },
      }),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(
      settings({ floatBarShowResetInline: true }),
    );

    const { container } = renderFloatBar(bootstrap({ floatBarShowResetInline: true }));
    await waitFor(() => {
      const pill = container.querySelector(".floatbar__pill");
      expect(pill?.getAttribute("title")).toContain("Claude: 80% used\nResets in 2 hours");
      expect(pill?.classList.contains("floatbar__pill--warn")).toBe(true);
      expect(container.querySelector(".floatbar__reset")?.textContent).toContain("2 hours");
    });
  });

  it("keeps an informational primary window when no secondary window is available", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("claude", "Claude", 10, { informational: true }),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings());

    const { container } = renderFloatBar(bootstrap());
    await waitFor(() => {
      expect(container.querySelector(".floatbar__pill")?.getAttribute("title")).toContain(
        "Claude: 10% used",
      );
    });
  });

  it("keeps an informational primary window when the secondary window is informational", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("claude", "Claude", 10, {
        informational: true,
        secondary: { used: 90, informational: true },
      }),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings());

    const { container } = renderFloatBar(bootstrap());
    await waitFor(() => {
      expect(container.querySelector(".floatbar__pill")?.getAttribute("title")).toContain(
        "Claude: 10% used",
      );
    });
  });

  it("sorts providers by their selected rate window", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("claude", "Claude", 90, {
        secondary: { used: 20 },
        selected: { used: 20 },
      }),
      snapshot("codex", "Codex", 50),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(
      settings({ providerMetrics: { claude: "weekly" } }),
    );

    const { container } = renderFloatBar(
      bootstrap({ providerMetrics: { claude: "weekly" } }),
    );
    await waitFor(() => {
      const titles = Array.from(container.querySelectorAll(".floatbar__pill")).map(
        (pill) => pill.getAttribute("title"),
      );
      expect(titles).toEqual(["Codex: 50% used", "Claude: 20% used"]);
    });
  });

  it("loads local cost summaries without using the foreground chart endpoint", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("codex", "Codex", 75),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings());
    tauriMocks.getProviderLocalUsageSummary.mockResolvedValue({
      todayCost: 1.25,
      thirtyDayCost: 12.5,
      thirtyDayTokens: 1000,
      latestTokens: 200,
      topModel: "gpt-5",
      estimateNote: "Estimated from local logs",
      tokenCostUpdatedAtMs: 1234,
    });

    renderFloatBar(bootstrap({ floatBarShowCost: true }));

    await waitFor(() => {
      expect(tauriMocks.getProviderLocalUsageSummary).toHaveBeenCalledWith("codex");
    });
    expect(tauriMocks.getProviderChartData).not.toHaveBeenCalled();
  });

  it("marks displayed local cost as an estimate", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([snapshot("codex", "Codex", 75)]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings({ floatBarShowCost: true }));
    tauriMocks.getProviderLocalUsageSummary.mockResolvedValue({
      todayCost: 1.25,
      thirtyDayCost: 12.5,
      thirtyDayTokens: 1000,
      latestTokens: 200,
      topModel: "gpt-5",
      estimateNote: "Estimated from local logs",
      tokenCostUpdatedAtMs: 1234,
    });

    const { container } = renderFloatBar(bootstrap({ floatBarShowCost: true }));

    await waitFor(() => {
      expect(container.querySelector(".floatbar__cost-estimate")?.textContent).toBe(
        "Estimate",
      );
    });
    expect(container.querySelector(".floatbar__cost-pill")?.getAttribute("title")).toContain(
      "(Estimate)",
    );
  });

  it("does not scan local costs by default", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("codex", "Codex", 75),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings());

    renderFloatBar(bootstrap());

    await waitFor(() => {
      expect(tauriMocks.getCachedProviders).toHaveBeenCalled();
    });
    expect(tauriMocks.getProviderLocalUsageSummary).not.toHaveBeenCalled();
    expect(document.querySelector(".floatbar__cost-pill")).toBeNull();
  });

  it("uses a safe state label instead of a raw provider error", async () => {
    const raw = "legacy telemetry failed for https://private.example.test; cookie=super-secret";
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("gemini", "Gemini", 12, { error: raw }),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings({ enabledProviders: ["gemini"] }));

    const { container } = renderFloatBar(bootstrap({ enabledProviders: ["gemini"] }));

    await waitFor(() => {
      const pill = container.querySelector(".floatbar__pill");
      expect(pill?.textContent).toContain("Legacy telemetry unavailable");
      expect(pill?.getAttribute("title")).toBe("Gemini: Legacy telemetry unavailable");
      expect(pill?.textContent).not.toContain("super-secret");
      expect(pill?.getAttribute("title")).not.toContain("private.example.test");
    });
  });

  it("can show remaining percentages when configured", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("claude", "Claude", 20),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings({ showAsUsed: false }));

    const { container } = renderFloatBar(bootstrap({ showAsUsed: false }));

    await waitFor(() => {
      const title = container
        .querySelector(".floatbar__pill")
        ?.getAttribute("title");
      expect(title).toContain("Claude: 80% remaining");
    });
  });

  it("applies warning tone when remaining drops below the high threshold", async () => {
    // highUsageThreshold = 70 → high-remaining cutoff = 30%.
    // claude at 80% used → 20% remaining → critical (also below crit cutoff 10).
    // Use 75% used → 25% remaining → warn (between 10 and 30).
    tauriMocks.getCachedProviders.mockResolvedValue([snapshot("claude", "Claude", 75)]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings());

    const { container } = renderFloatBar(bootstrap());
    await waitFor(() => {
      expect(container.querySelector(".floatbar__pill--warn")).not.toBeNull();
    });
  });

  it("applies critical tone when the provider is exhausted", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("claude", "Claude", 100, { exhausted: true }),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings());

    const { container } = renderFloatBar(bootstrap());
    await waitFor(() => {
      expect(container.querySelector(".floatbar__pill--crit")).not.toBeNull();
    });
  });

  it("filters to the floatBarProviderIds allowlist when non-empty", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("claude", "Claude", 30),
      snapshot("codex", "Codex", 50),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(
      settings({ floatBarProviderIds: ["codex"] }),
    );

    const { container } = renderFloatBar(
      bootstrap({ floatBarProviderIds: ["codex"] }),
    );
    await waitFor(() => {
      const pills = container.querySelectorAll(".floatbar__pill");
      expect(pills.length).toBe(1);
      expect(pills[0].getAttribute("title")).toMatch(/Codex/);
    });
  });

  it("does not show stale cached providers when all providers are disabled", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("claude", "Claude", 30),
      snapshot("codex", "Codex", 50),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(
      settings({ enabledProviders: [] }),
    );

    const { container } = renderFloatBar(bootstrap({ enabledProviders: [] }));
    await waitFor(() => {
      expect(container.querySelectorAll(".floatbar__pill").length).toBe(0);
      expect(container.querySelector(".floatbar__empty")).not.toBeNull();
    });
  });

  it("shows an empty state when no providers match", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings());

    const { container } = renderFloatBar(bootstrap());
    await waitFor(() => {
      expect(container.querySelector(".floatbar__empty")).not.toBeNull();
    });
  });

  it("applies the light-background class and CSS opacity", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(
      settings({ floatBarDarkText: true, floatBarOpacity: 45 }),
    );

    const { container } = renderFloatBar(
      bootstrap({ floatBarDarkText: true, floatBarOpacity: 45 }),
    );

    await waitFor(() => {
      const bar = container.querySelector<HTMLElement>(".floatbar");
      expect(bar).not.toBeNull();
      expect(bar?.classList.contains("floatbar--light-bg")).toBe(true);
      expect(bar?.style.opacity).toBe("0.45");
    });
  });

  it("applies the configured scale as a CSS variable", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings({ floatBarScale: 150 }));

    const { container } = renderFloatBar(bootstrap({ floatBarScale: 150 }));

    await waitFor(() => {
      const bar = container.querySelector<HTMLElement>(".floatbar");
      expect(bar).not.toBeNull();
      expect(bar?.style.getPropertyValue("--floatbar-scale")).toBe("1.5");
    });
  });

  it("resizes the native window in physical pixels at the WebView DPI", async () => {
    tauriMocks.getCachedProviders.mockResolvedValue([]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings());
    const originalDevicePixelRatio = window.devicePixelRatio;
    Object.defineProperty(window, "devicePixelRatio", {
      configurable: true,
      value: 1.5,
    });
    const rectSpy = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockReturnValue({
        x: 0,
        y: 0,
        width: 100,
        height: 20,
        top: 0,
        right: 100,
        bottom: 20,
        left: 0,
        toJSON: () => ({}),
      });

    try {
      renderFloatBar(bootstrap());

      await waitFor(() => {
        expect(coreMocks.invoke).toHaveBeenCalledWith("resize_float_bar", {
          width: 162,
          height: 42,
        });
      });
    } finally {
      rectSpy.mockRestore();
      Object.defineProperty(window, "devicePixelRatio", {
        configurable: true,
        value: originalDevicePixelRatio,
      });
    }
  });

  it("uses the localized reset formatter in pill tooltips", async () => {
    const resetsAt = new Date(Date.now() + 3 * 60 * 60_000 + 42 * 60_000).toISOString();
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("claude", "Claude", 20, { resetsAt }),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings());

    const { container } = renderFloatBar(bootstrap());

    await waitFor(() => {
      const title = container
        .querySelector(".floatbar__pill")
        ?.getAttribute("title");
      expect(title).toContain("Claude: 20% used");
      expect(title).toMatch(/Resets in 3h 4[12]m/);
      expect(title).not.toContain("Resets in due now");
    });
  });

  it("can render a next reset icon and time in provider pills", async () => {
    const resetsAt = new Date(Date.now() + 2 * 60 * 60_000 + 5 * 60_000).toISOString();
    tauriMocks.getCachedProviders.mockResolvedValue([
      snapshot("claude", "Claude", 20, { resetsAt }),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(
      settings({ floatBarShowResetInline: true }),
    );

    const { container } = renderFloatBar(
      bootstrap({ floatBarShowResetInline: true }),
    );

    await waitFor(() => {
      const reset = container.querySelector(".floatbar__reset");
      expect(reset).not.toBeNull();
      expect(reset?.getAttribute("aria-label")).toMatch(/Resets in 2h [45]m/);
      expect(reset?.textContent).toMatch(/2h [45]m/);
      expect(reset?.textContent).not.toContain("Resets in");
    });
  });

  it("polls refreshProvidersIfStale on the configured interval", async () => {
    vi.useFakeTimers();
    try {
      tauriMocks.getCachedProviders.mockResolvedValue([]);
      tauriMocks.getSettingsSnapshot.mockResolvedValue(settings());
      // 60s minimum is enforced in FloatBar.tsx; use the floor here.
      await act(async () => {
        renderFloatBar(bootstrap({ refreshIntervalSecs: 60 }));
      });

      // Initial tick fires synchronously on mount; useProviders is passive here
      // so the floatbar does not double-request stale refreshes at startup.
      await vi.waitFor(() => {
        expect(tauriMocks.refreshProvidersIfStale).toHaveBeenCalledTimes(1);
      });
      const initialCalls = tauriMocks.refreshProvidersIfStale.mock.calls.length;

      // Advance the timer past the 60-second interval — the floatbar tick
      // should fire again.
      await vi.advanceTimersByTimeAsync(60_000);
      expect(tauriMocks.refreshProvidersIfStale.mock.calls.length).toBeGreaterThan(
        initialCalls,
      );
    } finally {
      vi.useRealTimers();
    }
  });
});
