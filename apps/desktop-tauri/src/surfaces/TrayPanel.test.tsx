import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  getCachedProviders: vi.fn(),
  refreshProviders: vi.fn(),
  refreshProvidersIfStale: vi.fn(),
  getSettingsSnapshot: vi.fn(),
  updateSettings: vi.fn(),
  getUpdateState: vi.fn(),
  checkForUpdates: vi.fn(),
  downloadUpdate: vi.fn(),
  applyUpdate: vi.fn(),
  dismissUpdate: vi.fn(),
  openReleasePage: vi.fn(),
  setSurfaceMode: vi.fn(),
  dismissTrayPanel: vi.fn(),
  beginFlyoutGesture: vi.fn().mockResolvedValue(undefined),
  endFlyoutGesture: vi.fn().mockResolvedValue(undefined),
  openSettingsWindow: vi.fn(),
  quitApp: vi.fn(),
  getWorkAreaRect: vi.fn(),
  reanchorTrayPanel: vi.fn(),
  revealTrayPanelWindow: vi.fn(),
  flyoutStoredSize: vi.fn().mockResolvedValue(null),
  setFlyoutSize: vi.fn().mockResolvedValue(undefined),
  openProviderDashboard: vi.fn(),
  openProviderStatusPage: vi.fn(),
  getProviderChartData: vi.fn(),
  getCurrentSurfaceState: vi.fn(),
  getLocaleStrings: vi.fn(),
  setUiLanguage: vi.fn(),
  getDeepSeekPricingStatus: vi.fn().mockResolvedValue(null),
}));

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(),
  listeners: new Map<string, Array<(event: { payload: unknown }) => void>>(),
}));

const windowMocks = vi.hoisted(() => ({
  getCurrentWindow: vi.fn(() => ({
    setSize: vi.fn().mockResolvedValue(undefined),
    close: vi.fn().mockResolvedValue(undefined),
    scaleFactor: vi.fn().mockResolvedValue(1),
    onResized: vi.fn().mockResolvedValue(() => {}),
    innerSize: vi.fn().mockResolvedValue({ width: 328, height: 200 }),
  })),
  LogicalSize: vi.fn((width: number, height: number) => ({ width, height })),
  PhysicalSize: vi.fn((width: number, height: number) => ({ width, height })),
}));

vi.mock("../lib/tauri", () => tauriMocks);
vi.mock("@tauri-apps/api/event", () => eventMocks);
vi.mock("@tauri-apps/api/window", () => windowMocks);

import TrayPanel from "./TrayPanel";
import { LocaleProvider } from "../i18n/LocaleProvider";
import { TEST_PROVIDER_CATALOG } from "../test/providerCatalog";
import { buildBundle } from "../test/localeHarness";
import type {
  BootstrapState,
  ProviderCatalogEntry,
  ProviderUsageSnapshot,
  SettingsSnapshot,
} from "../types/bridge";

function rateWindow(used: number) {
  return {
    usedPercent: used,
    remainingPercent: 100 - used,
    windowMinutes: null,
    resetsAt: null,
    resetDescription: null,
    isExhausted: false,
    reservePercent: null,
    reserveDescription: null,
  };
}

function provider(id: string, displayName: string, used = 20): ProviderUsageSnapshot {
  return {
    providerId: id,
    displayName,
    primary: rateWindow(used),
    primaryLabel: "Monthly",
    secondary: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    cost: null,
    planName: null,
    accountEmail: null,
    sourceLabel: "auto",
    updatedAt: "2026-05-24T00:00:00Z",
    error: null,
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
    fetchDurationMs: null,
  };
}

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

function bootstrap(
  settingsOverrides: Partial<SettingsSnapshot> = {},
  catalog: ProviderCatalogEntry[] = [],
): BootstrapState {
  return {
    contractVersion: "v1",
    providers: catalog,
    settings: settings(settingsOverrides),
  };
}

function renderTrayPanel(
  providers: ProviderUsageSnapshot[],
  settingsOverrides: Partial<SettingsSnapshot> = {},
  catalog: ProviderCatalogEntry[] = [],
) {
  tauriMocks.getCachedProviders.mockResolvedValue(providers);
  tauriMocks.getSettingsSnapshot.mockResolvedValue(settings(settingsOverrides));
  return render(
    <LocaleProvider>
      <TrayPanel state={bootstrap(settingsOverrides, catalog)} />
    </LocaleProvider>,
  );
}

function emitEvent(event: string, payload: unknown) {
  for (const listener of eventMocks.listeners.get(event) ?? []) {
    listener({ payload });
  }
}

describe("TrayPanel provider grid", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    eventMocks.listeners.clear();
    tauriMocks.getDeepSeekPricingStatus.mockResolvedValue(null);
    tauriMocks.flyoutStoredSize.mockResolvedValue(null);
    tauriMocks.refreshProviders.mockResolvedValue(undefined);
    tauriMocks.refreshProvidersIfStale.mockResolvedValue(undefined);
    tauriMocks.dismissTrayPanel.mockResolvedValue(undefined);
    tauriMocks.reanchorTrayPanel.mockResolvedValue(undefined);
    tauriMocks.getWorkAreaRect.mockResolvedValue({
      x: 0,
      y: 0,
      width: 1440,
      height: 900,
    });
    tauriMocks.getCurrentSurfaceState.mockResolvedValue({
      mode: "trayPanel",
      target: { kind: "summary" },
    });
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings());
    tauriMocks.updateSettings.mockResolvedValue(settings());
    tauriMocks.getUpdateState.mockResolvedValue({
      status: "idle",
      version: null,
      error: null,
      progress: null,
      releaseUrl: null,
      canDownload: false,
      canApply: false,
      lastCheckedAt: null,
    });
    tauriMocks.getProviderChartData.mockResolvedValue({
      providerId: "codex",
      costHistory: [],
      creditsHistory: [],
      usageBreakdown: [],
      localUsage: null,
    });
    tauriMocks.getLocaleStrings.mockResolvedValue(
      buildBundle({
        ActionRefresh: "Refresh",
        MenuAbout: "About CodexBar",
        MenuQuit: "Quit",
        MenuSettings: "Settings...",
        PanelAllProviders: "All providers",
        PanelAllProvidersShort: "All",
        PanelLeftSuffix: "left",
        PanelShowAllProviders: "Show all providers",
        PanelShowFewerProviders: "Show fewer providers",
        PanelUsedSuffix: "used",
        PanelZoom: "Zoom",
      }),
    );
    eventMocks.listen.mockImplementation(
      (event: string, handler: (event: { payload: unknown }) => void) => {
        const listeners = eventMocks.listeners.get(event) ?? [];
        listeners.push(handler);
        eventMocks.listeners.set(event, listeners);
        return Promise.resolve(() => {});
      },
    );
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("reveals regardless of the shared surface-mode snapshot (TrayPanel now runs in its own dedicated window)", async () => {
    // TrayPanel is now hosted exclusively in the dedicated `flyout` OS
    // window (see App.tsx's isFlyoutWindow() routing), so it must not depend
    // on `main`'s surface-mode machine to know it's "open" — that machine
    // can never report "trayPanel" anymore (main only holds
    // Hidden/PopOut/Settings post-refactor). Overriding the snapshot mock to
    // something else confirms the fixed-size restore + reveal gate
    // (isFlyoutOpen, hardcoded true in TrayPanel.tsx) is no longer wired to
    // useSurfaceMode() at all.
    tauriMocks.getCurrentSurfaceState.mockResolvedValue({
      mode: "popOut",
      target: { kind: "dashboard" },
    });

    const { container } = renderTrayPanel([provider("claude", "Claude", 35)]);

    await waitFor(() => {
      expect(container.querySelector(".tray-panel-reveal--ready")).not.toBeNull();
    });
  });

  it("dismisses the tray panel on unmodified Escape", async () => {
    const { container } = renderTrayPanel([provider("claude", "Claude", 35)]);

    await waitFor(() => {
      expect(container.querySelector(".tray-panel-reveal--ready")).not.toBeNull();
    });

    fireEvent.keyDown(window, { key: "Escape" });

    await waitFor(() => {
      expect(tauriMocks.dismissTrayPanel).toHaveBeenCalledTimes(1);
    });
  });

  it("does not dismiss the tray panel on modified Escape", async () => {
    const { container } = renderTrayPanel([provider("claude", "Claude", 35)]);

    await waitFor(() => {
      expect(container.querySelector(".tray-panel-reveal--ready")).not.toBeNull();
    });

    fireEvent.keyDown(window, { key: "Escape", ctrlKey: true });
    fireEvent.keyDown(window, { key: "Escape", shiftKey: true });
    fireEvent.keyDown(window, { key: "Escape", altKey: true });
    fireEvent.keyDown(window, { key: "Escape", metaKey: true });

    expect(tauriMocks.dismissTrayPanel).not.toHaveBeenCalled();
  });

  it("keeps the existing Ctrl+R tray shortcut", async () => {
    const { container } = renderTrayPanel([provider("claude", "Claude", 35)]);

    await waitFor(() => {
      expect(container.querySelector(".tray-panel-reveal--ready")).not.toBeNull();
    });
    tauriMocks.refreshProviders.mockClear();

    fireEvent.keyDown(window, { key: "r", ctrlKey: true });

    await waitFor(() => {
      expect(tauriMocks.refreshProviders).toHaveBeenCalledTimes(1);
    });
  });

  it("localizes static tray panel labels in Japanese", async () => {
    tauriMocks.getLocaleStrings.mockResolvedValue(
      buildBundle(
        {
          ActionRefresh: "更新",
          MenuAbout: "CodexBar について",
          MenuQuit: "終了",
          MenuSettings: "設定...",
          PanelAllProviders: "すべてのプロバイダー",
          PanelAllProvidersShort: "すべて",
          PanelLatestTokens: "最新トークン",
          PanelThirtyDayCost: "30日間のコスト",
          PanelTopModelPrefix: "トップモデル",
          PanelEstimatedFromLocalLogs: "ローカルログから推定",
          PanelZoom: "ズーム",
          UpdatedDaysAgo: "{}日前",
        },
        "japanese",
      ),
    );
    tauriMocks.getProviderChartData.mockResolvedValue({
      providerId: "codex",
      costHistory: [{ date: "2026-05-24", value: 1.23 }],
      creditsHistory: [],
      usageBreakdown: [],
      localUsage: {
        todayCost: null,
        thirtyDayCost: 1.23,
        thirtyDayTokens: 584_000,
        latestTokens: 1200,
        topModel: "gpt-5.5",
        estimateNote: "Estimated from local logs",
        tokenCostUpdatedAtMs: 1234,
      },
    });

    const { container } = renderTrayPanel([provider("codex", "Codex", 35)]);

    await waitFor(() => {
      expect(
        container.querySelector('.provider-grid__item[aria-label="すべてのプロバイダー"]'),
      ).not.toBeNull();
    });
    expect(container.querySelector(".provider-grid__item")?.textContent).toContain("すべて");
    expect(screen.getByText("ズーム")).toBeInTheDocument();
    expect(screen.getByLabelText("ズーム")).toBeInTheDocument();
    expect(screen.getByText("更新")).toBeInTheDocument();
    expect(screen.getByText("設定...")).toBeInTheDocument();
    expect(screen.getByText("CodexBar について")).toBeInTheDocument();
    expect(screen.getByText("終了")).toBeInTheDocument();
    expect(await screen.findByText("30日間のコスト")).toBeInTheDocument();
    expect(container.querySelector(".menu-card__subtitle")?.textContent).toContain("日前");
    expect(screen.getByText("最新トークン")).toBeInTheDocument();
    expect(screen.getByText("トップモデル: gpt-5.5")).toBeInTheDocument();
    expect(screen.getByText("ローカルログから推定")).toBeInTheDocument();
  });

  it("localizes the expanded dense grid collapse label in Japanese", async () => {
    tauriMocks.getLocaleStrings.mockResolvedValue(
      buildBundle(
        {
          PanelAllProviders: "すべてのプロバイダー",
          PanelAllProvidersShort: "すべて",
          PanelShowAllProviders: "すべてのプロバイダーを表示",
          PanelShowFewerProviders: "表示を減らす",
        },
        "japanese",
      ),
    );
    const providers = TEST_PROVIDER_CATALOG.map(([id, displayName], index) =>
      provider(id, displayName, (index * 7) % 100),
    );

    const { container } = renderTrayPanel(providers);

    await waitFor(() => {
      expect(container.querySelector(".provider-grid--compact")).not.toBeNull();
    });

    fireEvent.click(
      container.querySelector<HTMLButtonElement>(
        '.provider-grid__item--more[aria-label="すべてのプロバイダーを表示"]',
      )!,
    );

    expect(await screen.findByText("表示を減らす")).toBeInTheDocument();
  });

  it.each([
    [1, true],
    [2, true],
    [5, true],
    [6, false],
    [12, false],
  ])("uses expected density for %i providers plus overview", async (providerCount, shouldBeSparse) => {
      const providers = [
        provider("codex", "Codex"),
        provider("claude", "Claude"),
        provider("copilot", "GitHub Copilot"),
        provider("cursor", "Cursor"),
        provider("gemini", "Gemini"),
        provider("kiro", "Kiro"),
        provider("zai", "z.ai"),
        provider("minimax", "MiniMax"),
        provider("vertexai", "Vertex AI"),
        provider("augment", "Augment"),
        provider("opencode", "OpenCode"),
        provider("kimi", "Kimi"),
      ].slice(0, providerCount);

      const { container } = renderTrayPanel(providers);

      await waitFor(() => {
        expect(container.querySelector(".provider-grid")).not.toBeNull();
      });

      const grid = container.querySelector(".provider-grid");
      expect(grid?.classList.contains("provider-grid--sparse")).toBe(
        shouldBeSparse,
      );
    },
  );

  it("only requests chart data for providers that can render charts", async () => {
    renderTrayPanel([
      provider("codex", "Codex"),
      provider("claude", "Claude"),
      provider("copilot", "GitHub Copilot"),
      provider("cursor", "Cursor"),
      provider("deepseek", "DeepSeek"),
    ]);

    await waitFor(() => {
      expect(tauriMocks.getProviderChartData).toHaveBeenCalledTimes(2);
    });

    expect(tauriMocks.getProviderChartData).toHaveBeenCalledWith("codex", undefined);
    expect(tauriMocks.getProviderChartData).toHaveBeenCalledWith("claude", undefined);
  });

  it("renders providers in settings catalog order instead of fetch completion order", async () => {
    const catalog: ProviderCatalogEntry[] = [
      { id: "codex", displayName: "Codex", cookieDomain: null },
      { id: "claude", displayName: "Claude", cookieDomain: null },
      { id: "cursor", displayName: "Cursor", cookieDomain: null },
      { id: "factory", displayName: "Factory", cookieDomain: null },
      { id: "gemini", displayName: "Gemini", cookieDomain: null },
    ];
    const providers = [
      provider("gemini", "Gemini", 10),
      provider("cursor", "Cursor", 20),
      { ...provider("codex", "Codex", 80), error: "Authentication required" },
      provider("factory", "Factory", 30),
      { ...provider("claude", "Claude", 40), error: "Claude sign-in missing" },
    ];

    const { container } = renderTrayPanel(
      providers,
      { enabledProviders: catalog.map((entry) => entry.id) },
      catalog,
    );

    await waitFor(() => {
      expect(container.querySelectorAll(".provider-grid__item")).toHaveLength(6);
    });

    const labels = Array.from(container.querySelectorAll(".provider-grid__item"))
      .map((node) => node.getAttribute("aria-label"));
    expect(labels).toEqual([
      "All providers",
      "Codex",
      "Claude",
      "Cursor",
      "Factory",
      "Gemini",
    ]);
    expect(
      Array.from(container.querySelectorAll(".menu-card__name")).map(
        (node) => node.textContent,
      ),
    ).toEqual(["Codex", "Claude", "Cursor", "Factory", "Gemini"]);
  });

  it("uses independent columns for a wide user-sized overview", async () => {
    tauriMocks.flyoutStoredSize.mockResolvedValue([700, 700]);
    const providers = [
      provider("codex", "Codex"),
      provider("claude", "Claude"),
      provider("antigravity", "Antigravity"),
      provider("copilot", "GitHub Copilot"),
    ];

    const { container } = renderTrayPanel(providers, {
      enabledProviders: providers.map((snapshot) => snapshot.providerId),
    });

    await waitFor(() => {
      expect(container.querySelector(".tray-panel-reveal--usersized")).not.toBeNull();
    });

    expect(
      Array.from(container.querySelectorAll(".menu-stack__column")).map((column) =>
        Array.from(column.querySelectorAll(".menu-stack__item")).map(
          (item) => item.id,
        ),
      ),
    ).toEqual([
      ["card-codex", "card-antigravity"],
      ["card-claude", "card-copilot"],
    ]);
    expect(container.querySelector(".menu-stack__sep")).toBeNull();
  });

  it("keeps the stacked layout when the saved flyout width is narrow", async () => {
    vi.spyOn(window, "innerWidth", "get").mockReturnValue(700);
    tauriMocks.flyoutStoredSize.mockResolvedValue([500, 700]);
    const providers = [
      provider("codex", "Codex"),
      provider("claude", "Claude"),
      provider("antigravity", "Antigravity"),
      provider("copilot", "GitHub Copilot"),
    ];

    const { container } = renderTrayPanel(providers, {
      enabledProviders: providers.map((snapshot) => snapshot.providerId),
    });

    await waitFor(() => {
      expect(container.querySelector(".tray-panel-reveal--usersized")).not.toBeNull();
    });

    expect(container.querySelector(".menu-stack__column")).toBeNull();
    expect(container.querySelectorAll(".menu-stack__sep")).toHaveLength(3);
  });

  it("collapses and expands the full provider catalog in the dense tray grid", async () => {
    const providers = TEST_PROVIDER_CATALOG.map(([id, displayName], index) =>
      provider(id, displayName, (index * 7) % 100),
    );

    const { container } = renderTrayPanel(providers);

    await waitFor(() => {
      expect(container.querySelectorAll(".provider-grid__item")).toHaveLength(
        20,
      );
    });

    const grid = container.querySelector(".provider-grid");
    expect(grid?.classList.contains("provider-grid--sparse")).toBe(false);
    expect(grid?.classList.contains("provider-grid--compact")).toBe(true);
    expect(grid?.getAttribute("data-expanded")).toBe("false");
    expect(grid?.getAttribute("data-provider-count")).toBe(
      String(providers.length + 1),
    );
    expect(container.querySelectorAll(".menu-stack__item")).toHaveLength(4);

    const expand = container.querySelector<HTMLButtonElement>(
      '.provider-grid__item--more[aria-label="Show all providers"]',
    );
    expect(expand).not.toBeNull();
    expect(expand?.textContent).toContain(`+${providers.length - 18}`);

    fireEvent.click(expand!);

    await waitFor(() => {
      expect(container.querySelectorAll(".provider-grid__item")).toHaveLength(
        providers.length + 2,
      );
    });
    expect(grid?.getAttribute("data-expanded")).toBe("true");
    expect(container.querySelectorAll(".menu-stack__item")).toHaveLength(
      providers.length,
    );
    for (const [id, displayName] of TEST_PROVIDER_CATALOG) {
      expect(
        container.querySelector(`.provider-grid__item[aria-label="${displayName}"]`),
        id,
      ).not.toBeNull();
    }
  });

  it("uses compact provider labels for huge catalogs without losing full accessible labels", async () => {
    const providers = TEST_PROVIDER_CATALOG.slice(0, 36).map(
      ([id, displayName], index) => provider(id, displayName, (index * 7) % 100),
    );

    const { container } = renderTrayPanel(providers);

    await waitFor(() => {
      expect(container.querySelector(".provider-grid--compact")).not.toBeNull();
    });

    const expand = container.querySelector<HTMLButtonElement>(
      '.provider-grid__item--more[aria-label="Show all providers"]',
    );
    expect(expand).not.toBeNull();

    fireEvent.click(expand!);

    await waitFor(() => {
      expect(
        container.querySelector('.provider-grid__item[aria-label="Copilot"]'),
      ).not.toBeNull();
    });

    const copilot = container.querySelector(
      '.provider-grid__item[aria-label="Copilot"]',
    );
    expect(copilot).not.toBeNull();
    expect(copilot?.getAttribute("aria-label")).toBe("Copilot");
    expect(copilot?.querySelector(".provider-grid__label")?.textContent).toBe(
      "Copi",
    );
  });

  it("provider grid indicator follows the show-as-used setting", async () => {
    const { container, rerender } = renderTrayPanel(
      [provider("claude", "Claude", 35)],
      { showAsUsed: true },
    );

    await waitFor(() => {
      const track = container.querySelector<HTMLElement>(
        ".provider-grid__weekly-track",
      );
      expect(track?.style.getPropertyValue("--weekly-pct")).toBe("35%");
    });

    tauriMocks.getCachedProviders.mockResolvedValue([
      provider("claude", "Claude", 35),
    ]);
    tauriMocks.getSettingsSnapshot.mockResolvedValue(settings({ showAsUsed: false }));
    rerender(
      <LocaleProvider>
        <TrayPanel state={bootstrap({ showAsUsed: false })} />
      </LocaleProvider>,
    );

    await waitFor(() => {
      const track = container.querySelector<HTMLElement>(
        ".provider-grid__weekly-track",
      );
      expect(track?.style.getPropertyValue("--weekly-pct")).toBe("65%");
    });
  });

  it("hides provider grid icons when the display setting is disabled", async () => {
    const { container } = renderTrayPanel(
      [provider("codex", "Codex"), provider("claude", "Claude")],
      { switcherShowsIcons: false },
    );

    await waitFor(() => {
      expect(container.querySelector(".provider-grid")).not.toBeNull();
    });

    const grid = container.querySelector(".provider-grid");
    expect(grid?.getAttribute("data-show-icons")).toBe("false");
    expect(grid?.classList.contains("provider-grid--no-icons")).toBe(true);
    expect(container.querySelector(".provider-icon")).toBeNull();
    expect(container.querySelector(".provider-grid__icon-overview")).toBeNull();
  });

  it("renders the tray footer zoom slider above Refresh and persists trayScalePercent after the debounce", async () => {
    const { container } = renderTrayPanel(
      [provider("claude", "Claude", 35)],
      { trayScalePercent: 120 },
    );

    await waitFor(() => {
      expect(container.querySelector(".menu-surface__footer-zoom")).not.toBeNull();
    });

    const footerChildren = Array.from(
      container.querySelectorAll(".menu-surface__footer > *"),
    );
    const zoomIndex = footerChildren.findIndex((el) =>
      el.classList.contains("menu-surface__footer-zoom"),
    );
    const refreshIndex = footerChildren.findIndex(
      (el) => el.textContent?.includes("Refresh"),
    );
    expect(zoomIndex).toBeGreaterThanOrEqual(0);
    expect(refreshIndex).toBeGreaterThan(zoomIndex);

    // Slider reflects the persisted settings value.
    const slider = container.querySelector<HTMLInputElement>(
      ".menu-surface__footer-zoom-slider",
    )!;
    expect(slider).not.toBeNull();
    expect(slider.value).toBe("120");
    expect(slider.min).toBe("100");
    expect(slider.max).toBe("200");
    expect(slider.step).toBe("5");
    expect(
      container.querySelector(".menu-surface__footer-zoom-value")?.textContent,
    ).toBe("120%");

    fireEvent.change(slider, { target: { value: "150" } });

    // Live preview: thumb and readout update immediately from local state…
    expect(slider.value).toBe("150");
    expect(
      container.querySelector(".menu-surface__footer-zoom-value")?.textContent,
    ).toBe("150%");

    // …while persistence trails the ~250ms debounce (not synchronous).
    expect(tauriMocks.updateSettings).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(tauriMocks.updateSettings).toHaveBeenCalledWith({
        trayScalePercent: 150,
      });
    });
    expect(tauriMocks.updateSettings).toHaveBeenCalledTimes(1);
  });

  it("reveals the tray panel if the native resize pass fails", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    windowMocks.getCurrentWindow.mockReturnValue({
      setSize: vi.fn().mockRejectedValue(new Error("resize failed")),
      close: vi.fn().mockResolvedValue(undefined),
      scaleFactor: vi.fn().mockResolvedValue(1),
      onResized: vi.fn().mockResolvedValue(() => {}),
      innerSize: vi.fn().mockResolvedValue({ width: 328, height: 200 }),
    });

    const { container } = renderTrayPanel([provider("claude", "Claude", 35)]);

    await waitFor(() => {
      expect(container.querySelector(".tray-panel-reveal--ready")).not.toBeNull();
    });

    warn.mockRestore();
  });

  it("does not resize the native tray window for usage-only provider updates", async () => {
    const setSize = vi.fn().mockResolvedValue(undefined);
    windowMocks.getCurrentWindow.mockReturnValue({
      setSize,
      close: vi.fn().mockResolvedValue(undefined),
      scaleFactor: vi.fn().mockResolvedValue(1),
      onResized: vi.fn().mockResolvedValue(() => {}),
      innerSize: vi.fn().mockResolvedValue({ width: 328, height: 200 }),
    });

    const { container } = renderTrayPanel([provider("claude", "Claude", 35)]);

    await waitFor(() => {
      expect(container.querySelector(".tray-panel-reveal--ready")).not.toBeNull();
    });
    setSize.mockClear();
    tauriMocks.reanchorTrayPanel.mockClear();

    act(() => {
      emitEvent("provider-updated", provider("claude", "Claude", 52));
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 200));
    });

    expect(setSize).not.toHaveBeenCalled();
    expect(tauriMocks.reanchorTrayPanel).not.toHaveBeenCalled();
  });

  it("reserves dense all-provider height on first layout", async () => {
    const setSize = vi.fn().mockResolvedValue(undefined);
    windowMocks.getCurrentWindow.mockReturnValue({
      setSize,
      close: vi.fn().mockResolvedValue(undefined),
      scaleFactor: vi.fn().mockResolvedValue(1),
      onResized: vi.fn().mockResolvedValue(() => {}),
      innerSize: vi.fn().mockResolvedValue({ width: 328, height: 200 }),
    });
    const denseProviders = TEST_PROVIDER_CATALOG.slice(0, 36).map(([id, displayName]) =>
      provider(id, displayName),
    );

    renderTrayPanel(denseProviders, {
      enabledProviders: denseProviders.map((snapshot) => snapshot.providerId),
    });

    await waitFor(() => {
      expect(setSize).toHaveBeenCalledWith(
        expect.objectContaining({ width: 328, height: 776 }),
      );
    });
  });

  it("keeps provider detail mode tall enough for context actions and footer", async () => {
    const setSize = vi.fn().mockResolvedValue(undefined);
    windowMocks.getCurrentWindow.mockReturnValue({
      setSize,
      close: vi.fn().mockResolvedValue(undefined),
      scaleFactor: vi.fn().mockResolvedValue(1),
      onResized: vi.fn().mockResolvedValue(() => {}),
      innerSize: vi.fn().mockResolvedValue({ width: 328, height: 200 }),
    });
    const errorProvider = {
      ...provider("abacus", "Abacus AI", 0),
      error: "Source mode `Cli` not supported for this provider",
    };

    const { container } = renderTrayPanel([errorProvider]);

    await waitFor(() => {
      expect(container.querySelector(".tray-panel-reveal--ready")).not.toBeNull();
    });
    setSize.mockClear();

    fireEvent.click(
      container.querySelector<HTMLButtonElement>(
        '.provider-grid__item[aria-label="Abacus AI"]',
      )!,
    );

    await waitFor(() => {
      expect(setSize).toHaveBeenCalledWith(
        expect.objectContaining({ width: 328, height: 420 }),
      );
    });
  });

  it("scales the auto-fit measure by the active tray zoom before clamping (#265)", async () => {
    const setSize = vi.fn().mockResolvedValue(undefined);
    windowMocks.getCurrentWindow.mockReturnValue({
      setSize,
      close: vi.fn().mockResolvedValue(undefined),
      scaleFactor: vi.fn().mockResolvedValue(1),
      onResized: vi.fn().mockResolvedValue(() => {}),
      innerSize: vi.fn().mockResolvedValue({ width: 328, height: 200 }),
    });
    // jsdom has no layout engine (scrollHeight always reads 0), so pin it
    // globally to a deterministic PRE-zoom content height: TrayPanel applies
    // `zoom: trayScale` via CSS and the hook must size the window in POST-zoom
    // px or tall cards clip below the fold (#265).
    const scrollHeight = vi
      .spyOn(Element.prototype, "scrollHeight", "get")
      .mockReturnValue(505);

    const first = renderTrayPanel([provider("codex", "Codex", 61)], {
      trayScalePercent: 150,
    });

    // 505 raw × 1.5 zoom = 757.5 → 758 rounded, + the 4px fudge = 762.
    await waitFor(() => {
      expect(setSize).toHaveBeenCalledWith(
        expect.objectContaining({ width: 328, height: 762 }),
      );
    });
    first.unmount();
    setSize.mockClear();

    // Same zoom, taller content: round(700 × 1.5) + 4 = 1054 exceeds the
    // mocked work-area cap (900 - 16 = 884), so the clamp still wins.
    scrollHeight.mockReturnValue(700);
    renderTrayPanel([provider("codex", "Codex", 61)], {
      trayScalePercent: 150,
    });

    await waitFor(() => {
      expect(setSize).toHaveBeenCalledWith(
        expect.objectContaining({ width: 328, height: 884 }),
      );
    });
  });
});
