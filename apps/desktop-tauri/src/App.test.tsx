import { render, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

// App.tsx routes by `getCurrentWebviewWindow().label` before falling through
// to the shared-window SurfaceRouter (which reads the surface-mode snapshot
// instead). These tests focus on that routing decision — which top-level
// surface App mounts for a given window label — not on any individual
// surface's internal behavior (those have their own dedicated test files).

const webviewWindowMocks = vi.hoisted(() => ({
  label: "main",
}));

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: webviewWindowMocks.label }),
}));

const tauriMocks = vi.hoisted(() => ({
  getBootstrapState: vi.fn(),
  getSettingsSnapshot: vi.fn(),
  checkForUpdates: vi.fn(),
  downloadUpdate: vi.fn(),
  setSurfaceMode: vi.fn(),
  getLocaleStrings: vi.fn(),
  setUiLanguage: vi.fn(),
  getCurrentSurfaceState: vi.fn(),
}));

vi.mock("./lib/tauri", () => tauriMocks);

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => eventMocks);

// Stand-in surfaces: assert routing, not each surface's own rendering.
vi.mock("./surfaces/TrayPanel", () => ({
  default: () => <div data-testid="surface-tray-panel" />,
}));
vi.mock("./surfaces/PopOutPanel", () => ({
  default: () => <div data-testid="surface-pop-out-panel" />,
}));
vi.mock("./surfaces/Settings", () => ({
  default: () => <div data-testid="surface-settings" />,
}));
vi.mock("./floatbar/FloatBar", () => ({
  default: () => <div data-testid="surface-float-bar" />,
}));

vi.mock("./hooks/useSurfaceSnapshot", () => ({
  useSurfaceSnapshot: () => ({
    mode: "hidden",
    target: { kind: "summary" },
  }),
}));

import App from "./App";
import { buildBundle } from "./test/localeHarness";
import type { BootstrapState, SettingsSnapshot } from "./types/bridge";

function settings(overrides: Partial<SettingsSnapshot> = {}): SettingsSnapshot {
  return {
    enabledProviders: ["codex", "claude"],
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
    // "dark" (not "auto") so useTheme's effect short-circuits before ever
    // touching window.matchMedia, which jsdom doesn't implement here.
    theme: "dark",
    windowScalePercent: 125,
    trayScalePercent: 100,
    powertoysStatusPipeEnabled: false,
    claudeAvoidKeychainPrompts: false,
    codexSparkUsageVisible: true,
    disableKeychainAccess: false,
    providerMetrics: {},
    floatBarEnabled: false,
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
    alibabaTokenPlanRegion: "cn",
    weeklyProgressWorkDays: null,
    ...overrides,
  };
}

function bootstrap(): BootstrapState {
  return {
    contractVersion: "v1",
    providers: [],
    settings: settings(),
  };
}

describe("App window-label routing", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    webviewWindowMocks.label = "main";
    tauriMocks.getBootstrapState.mockResolvedValue(bootstrap());
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings());
    tauriMocks.checkForUpdates.mockResolvedValue({
      status: "idle",
      version: null,
      error: null,
      progress: null,
      releaseUrl: null,
      canDownload: false,
      canApply: false,
      lastCheckedAt: null,
    });
    tauriMocks.getLocaleStrings.mockResolvedValue(buildBundle());
    tauriMocks.getCurrentSurfaceState.mockResolvedValue({
      mode: "hidden",
      target: { kind: "summary" },
    });
    eventMocks.listen.mockResolvedValue(() => {});
  });

  it("routes the dedicated flyout window to TrayPanel", async () => {
    webviewWindowMocks.label = "flyout";

    const { queryByTestId } = render(<App />);

    await waitFor(() => {
      expect(queryByTestId("surface-tray-panel")).not.toBeNull();
    });
    expect(queryByTestId("surface-pop-out-panel")).toBeNull();
    expect(queryByTestId("surface-settings")).toBeNull();
    expect(queryByTestId("surface-float-bar")).toBeNull();
  });

  it("routes the detached settings window to Settings, not TrayPanel", async () => {
    webviewWindowMocks.label = "settings";

    const { queryByTestId } = render(<App />);

    await waitFor(() => {
      expect(queryByTestId("surface-settings")).not.toBeNull();
    });
    expect(queryByTestId("surface-tray-panel")).toBeNull();
  });

  it("routes the detached floatbar window to FloatBar, not TrayPanel", async () => {
    webviewWindowMocks.label = "floatbar";

    const { queryByTestId } = render(<App />);

    await waitFor(() => {
      expect(queryByTestId("surface-float-bar")).not.toBeNull();
    });
    expect(queryByTestId("surface-tray-panel")).toBeNull();
  });

  it("does not route the shared main window to TrayPanel while hidden", async () => {
    // main's surface-mode machine only ever holds Hidden/PopOut/Settings
    // post-refactor — it can never report "trayPanel" — so the
    // isFlyoutWindow()/isSettingsWindow()/isFloatBarWindow() checks all miss
    // and control falls through to SurfaceRouter, which renders nothing for
    // "hidden".
    webviewWindowMocks.label = "main";

    const { container, queryByTestId } = render(<App />);

    // Wait for BOTH the bootstrap state AND the locale bundle to resolve —
    // AppInner only reaches the isFlyoutWindow()/SurfaceRouter branch after
    // `state` is set, and LocaleProvider only renders children after its own
    // bundle loads. Waiting for both pushes past every loading-placeholder
    // return path, so a null firstChild here reflects the settled "hidden"
    // SurfaceRouter branch, not an earlier loading state.
    await waitFor(() => {
      expect(tauriMocks.getBootstrapState).toHaveBeenCalled();
      expect(tauriMocks.getLocaleStrings).toHaveBeenCalled();
    });
    await waitFor(() => {
      expect(container.querySelector("main.shell")).toBeNull();
    });
    expect(queryByTestId("surface-tray-panel")).toBeNull();
    expect(container.firstChild).toBeNull();
  });
});
