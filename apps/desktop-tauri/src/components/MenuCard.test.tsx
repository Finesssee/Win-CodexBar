import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  getProviderChartData: vi.fn(),
  getDeepSeekPricingStatus: vi.fn(),
  getLocaleStrings: vi.fn(),
  setUiLanguage: vi.fn(),
}));

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(),
}));

vi.mock("../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/tauri")>()),
  ...tauriMocks,
}));
vi.mock("@tauri-apps/api/event", () => eventMocks);

import { LocaleProvider } from "../i18n/LocaleProvider";
import { buildBundle } from "../test/localeHarness";
import type { ProviderUsageSnapshot } from "../types/bridge";
import MenuCard from "./MenuCard";

function rateWindow(
  usedPercent = 0,
  opts: {
    exhausted?: boolean;
    resetDescription?: string | null;
    reservePercent?: number | null;
    reserveDescription?: string | null;
    reserveWillLastToReset?: boolean;
    reserveEtaSeconds?: number | null;
    windowMinutes?: number | null;
    resetsAt?: string | null;
  } = {},
) {
  return {
    usedPercent,
    remainingPercent: 100 - usedPercent,
    windowMinutes: opts.windowMinutes ?? null,
    resetsAt: opts.resetsAt ?? null,
    resetDescription: opts.resetDescription ?? null,
    isExhausted: opts.exhausted ?? false,
    reservePercent: opts.reservePercent ?? null,
    reserveDescription: opts.reserveDescription ?? null,
    reserveWillLastToReset: opts.reserveWillLastToReset ?? false,
    reserveEtaSeconds: opts.reserveEtaSeconds ?? null,
  };
}

function provider(
  error: string | null,
  usedPercent = 0,
  opts: { exhausted?: boolean; resetDescription?: string | null } = {},
): ProviderUsageSnapshot {
  return {
    providerId: "claude",
    displayName: "Claude",
    primary: rateWindow(usedPercent, opts),
    selectedMetric: rateWindow(usedPercent, opts),
    primaryLabel: "Session",
    secondary: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    cost: null,
    planName: null,
    accountEmail: null,
    sourceLabel: "oauth",
    updatedAt: "2026-05-24T00:00:00Z",
    error,
    errorState: "unknown",
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
    fetchDurationMs: null,
  };
}

function renderCard(
  snapshot: ProviderUsageSnapshot,
  opts: {
    showAsUsed?: boolean;
    showResetWhenExhausted?: boolean;
    showPace?: boolean;
    onLayoutChange?: () => void;
    costSummaryDisplayStyle?: "compact" | "detailed" | "hidden";
  } = {},
) {
  return render(
    <LocaleProvider>
      <MenuCard
        provider={snapshot}
        display={{
          hideEmail: false,
          resetTimeRelative: true,
          showAsUsed: opts.showAsUsed,
          showResetWhenExhausted: opts.showResetWhenExhausted,
          showPace: opts.showPace,
          costSummaryDisplayStyle: opts.costSummaryDisplayStyle,
        }}
        onLayoutChange={opts.onLayoutChange}
      />
    </LocaleProvider>,
  );
}

describe("MenuCard", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.getLocaleStrings.mockResolvedValue(
      buildBundle({
        ActionCopyError: "Copy error",
        ApiSpendTitle: "API spend",
        DetailPaceRunsOutIn: "Runs out in",
        PanelEstimatedFromLocalLogs: "Estimated from local logs",
        PanelLeftSuffix: "left",
        PanelNow: "now",
        PanelOneHour: "1h",
        PanelFiveHours: "5h",
        PanelOnPaceBudget: "On-pace budget",
        PanelReserveSuffix: "in reserve",
        PanelThirtyDayCost: "30d cost",
        PanelThirtyDayTokens: "30d tokens",
        PanelTodayBudget: "today",
        PanelUsedSuffix: "used",
        ResetsInHoursMinutes: "Resets in {}h {}m",
        ResetsInMinutes: "Resets in {}m",
        ResetsInDaysHours: "Resets in {}d {}h",
        NextExpiresInHoursMinutes: "Next expires in {}h {}m",
        NextExpiresInMinutes: "Next expires in {}m",
        NextExpiresInDaysHours: "Next expires in {}d {}h",
        NextExpiresDueNow: "Expires now",
        WayfinderGatewayStatus: "Gateway",
        WayfinderModels: "Models",
        WayfinderRequests: "Requests",
        WayfinderTokens: "Tokens",
        WayfinderSaved: "Saved",
        WayfinderOffline: "Gateway offline",
        WayfinderDryRun: "Dry run",
        WayfinderMissingKeys: "Missing keys",
        DeepSeekPricingTitle: "DeepSeek pricing",
        DeepSeekPricingStandard: "Standard / pre-schedule",
        DeepSeekPricingPeak: "Peak hours",
        DeepSeekPricingOffPeak: "Off-peak hours",
        DeepSeekPricingCurrent: "Current local time:",
        DeepSeekPricingNext: "Next transition:",
        DeepSeekPricingEffective: "Effective local time:",
        DeepSeekPricingAdvice: "Official schedule",
      }),
    );
    tauriMocks.getDeepSeekPricingStatus.mockResolvedValue(null);
    tauriMocks.getProviderChartData.mockResolvedValue({
      providerId: "claude",
      costHistory: [{ date: "2026-05-24", value: 1.23 }],
      creditsHistory: [],
      usageBreakdown: [],
      localUsage: {
        todayCost: null,
        thirtyDayCost: 1.23,
        thirtyDayTokens: 584_000,
        latestTokens: null,
        topModel: "glim-4.6",
        estimateNote: "Estimated from local logs",
        tokenCostUpdatedAtMs: 1234,
      },
    });
    eventMocks.listen.mockResolvedValue(() => {});
  });

  it("keeps Fireworks vendor API spend visible when local cost summaries are hidden", async () => {
    const snapshot = provider(null, 0);
    snapshot.providerId = "fireworks";
    snapshot.displayName = "Fireworks";
    snapshot.cost = {
      used: 12.34,
      limit: null,
      remaining: null,
      currencyCode: "USD",
      currencySymbol: "$",
      period: "30 days",
      resetsAt: null,
      formattedUsed: "$12.34",
      formattedLimit: null,
      balance: null,
      formattedBalance: null,
      daily: [],
      alwaysVisible: true,
    };

    renderCard(snapshot, { costSummaryDisplayStyle: "hidden" });

    expect(await screen.findByText("API spend")).toBeInTheDocument();
    expect(document.querySelector(".menu-card__cost-line")).toHaveTextContent("$12.34");
  });
  it("does not mix stale local usage into an error card", async () => {
    const { container } = renderCard(
      provider("OAuth error: Claude OAuth credentials not found."),
    );

    expect(
      await screen.findByText("OAuth error: Claude OAuth credentials not found."),
    ).toBeInTheDocument();
    expect(container.querySelector(".menu-card--header-only")).toBeInTheDocument();
    expect(container.querySelector(".menu-card--with-details")).not.toBeInTheDocument();

    await waitFor(() => {
      expect(tauriMocks.getProviderChartData).toHaveBeenCalled();
    });

    expect(screen.queryByText("30d cost")).not.toBeInTheDocument();
    expect(screen.queryByText("30d tokens")).not.toBeInTheDocument();
    expect(screen.queryByText("Estimated from local logs")).not.toBeInTheDocument();
  });

  it("shows DeepSeek peak/off-peak pricing status", async () => {
    tauriMocks.getDeepSeekPricingStatus.mockResolvedValue({
      period: "offPeak",
      currentLocalTime: "2026-08-17 05:00:00 UTC",
      nextTransitionLocalTime: "2026-08-17 06:00:00 UTC",
      effectiveLocalTime: "2026-08-16 18:00:00 EDT",
    });
    const snapshot = provider(null);
    snapshot.providerId = "deepseek";
    snapshot.displayName = "DeepSeek";

    renderCard(snapshot);

    expect(await screen.findByText("DeepSeek pricing: Off-peak hours")).toBeInTheDocument();
    expect(screen.getByText("Current local time: 2026-08-17 05:00:00 UTC")).toBeInTheDocument();
    expect(screen.getByText("Next transition: 2026-08-17 06:00:00 UTC")).toBeInTheDocument();
  });

  it("can render metric bars as used instead of remaining", async () => {
    renderCard(provider(null, 35), { showAsUsed: true });

    expect(await screen.findByText("35% used")).toBeInTheDocument();
    expect(screen.queryByText("65% left")).not.toBeInTheDocument();

    const fill = document.querySelector<HTMLElement>(".menu-metric__bar-fill");
    expect(fill?.style.width).toBe("35%");
  });

  it("displays over-quota usage without overflowing the bar", async () => {
    renderCard(provider(null, 115, { exhausted: true, resetDescription: "115% used" }), {
      showAsUsed: true,
    });

    expect(await screen.findAllByText("115% used")).not.toHaveLength(0);
    const fill = document.querySelector<HTMLElement>(".menu-metric__bar-fill");
    expect(fill?.style.width).toBe("100%");
  });

  it("replaces an exhausted percentage with a future reset countdown", async () => {
    const snapshot = provider(null, 100, { exhausted: true });
    snapshot.primary.resetsAt = new Date(Date.now() + 60 * 60 * 1000).toISOString();

    renderCard(snapshot, { showResetWhenExhausted: true });

    expect(await screen.findByText(/Resets in \d+m/)).toBeInTheDocument();
    expect(screen.queryByText("0% left")).not.toBeInTheDocument();
  });

  it("keeps an exhausted percentage without a concrete future reset", async () => {
    renderCard(provider(null, 100, { exhausted: true, resetDescription: "in 2h" }), {
      showResetWhenExhausted: true,
    });

    expect(await screen.findByText("0% left")).toBeInTheDocument();
  });

  it("renders additional Copilot budget windows", async () => {
    const snapshot = provider(null, 20);
    snapshot.providerId = "copilot";
    snapshot.displayName = "GitHub Copilot";
    snapshot.extraRateWindows = [
      {
        id: "additional_budget",
        title: "Additional Budget",
        window: rateWindow(42),
      },
    ];

    renderCard(snapshot);

    expect(await screen.findByText("Additional Budget")).toBeInTheDocument();
    expect(screen.getByText("58% left")).toBeInTheDocument();
  });

  it("renders informational metrics without quota percentages", async () => {
    const snapshot = provider(null, 20);
    snapshot.extraRateWindows = [
      {
        id: "requests",
        title: "Requests",
        window: {
          ...rateWindow(0),
          isInformational: true,
          resetDescription: "7 requests",
        },
      },
    ];

    renderCard(snapshot);

    const title = await screen.findByText("Requests");
    expect(title.parentElement).not.toHaveTextContent("100% left");
    expect(title.parentElement?.querySelector(".menu-metric__bar")).toBeNull();
    expect(screen.getByText("7 requests")).toBeInTheDocument();
  });

  it("shows reset credit count and next expiry without a percent bar", async () => {
    const snapshot = provider(null, 20);
    snapshot.extraRateWindows = [
      {
        id: "reset-credits",
        title: "Reset credits",
        window: {
          ...rateWindow(0, {
            resetsAt: new Date(Date.now() + 6 * 24 * 60 * 60 * 1000 + 21 * 60 * 60 * 1000).toISOString(),
            resetDescription: "2 reset credits available",
          }),
          isInformational: true,
        },
      },
    ];

    renderCard(snapshot);

    const title = await screen.findByText("Reset credits");
    expect(screen.getByText("2 reset credits available")).toBeInTheDocument();
    expect(screen.getByText(/Next expires in/)).toBeInTheDocument();
    expect(title.parentElement?.querySelector(".menu-metric__bar")).toBeNull();
  });

  it("keeps reset credit count in absolute reset-time mode", async () => {
    const snapshot = provider(null, 20);
    snapshot.extraRateWindows = [
      {
        id: "reset-credits",
        title: "Reset credits",
        window: {
          ...rateWindow(0, {
            resetsAt: new Date(Date.now() + 6 * 24 * 60 * 60 * 1000).toISOString(),
            resetDescription: "2 reset credits available",
          }),
          isInformational: true,
        },
      },
    ];

    render(
      <LocaleProvider>
        <MenuCard
          provider={snapshot}
          display={{
            hideEmail: false,
            resetTimeRelative: false,
          }}
        />
      </LocaleProvider>,
    );

    expect(await screen.findByText("2 reset credits available")).toBeInTheDocument();
    expect(screen.queryByText(/Next expires in/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Resets in/)).not.toBeInTheDocument();
  });

  it("renders Wayfinder telemetry without quota or identity rows", async () => {
    const snapshot = provider(null);
    snapshot.providerId = "wayfinder";
    snapshot.displayName = "Wayfinder";
    snapshot.accountEmail = "should-not-render@example.test";
    snapshot.planName = "should-not-render";
    snapshot.wayfinderUsage = {
      gatewayStatus: "ok",
      offline: false,
      dryRun: false,
      missingKeys: [],
      modelCount: 2,
      models: ["model-a", "model-b"],
      requests: 14,
      estimatedRequests: 0,
      tokens: 1028,
      realized: 0.004,
      baseline: 0.01,
      saved: 0.006,
      savedPercent: 60,
      periodDays: 30,
      unit: "usd",
      priced: true,
      routes: [],
    };

    renderCard(snapshot);

    expect(await screen.findByText("ok")).toBeInTheDocument();
    expect(screen.getByText("2")).toBeInTheDocument();
    expect(screen.getByText("1K")).toBeInTheDocument();
    expect(screen.queryByText("should-not-render@example.test")).not.toBeInTheDocument();
    expect(screen.queryByText("should-not-render")).not.toBeInTheDocument();
    expect(screen.queryByText("Session")).not.toBeInTheDocument();
  });

  it("uses explicit hourly quota labels in Simplified Chinese", async () => {
    tauriMocks.getLocaleStrings.mockResolvedValue(buildBundle({}, "chinese"));
    const snapshot = provider(null, 31);
    snapshot.primary = rateWindow(31, { windowMinutes: 3 * 60 });
    snapshot.selectedMetric = snapshot.primary;

    renderCard(snapshot);

    expect(await screen.findByText("3 小时")).toBeInTheDocument();
    expect(screen.queryByText("Session")).not.toBeInTheDocument();
  });

  it("maps a Simplified Chinese session-labelled weekly window to Weekly", async () => {
    tauriMocks.getLocaleStrings.mockResolvedValue(
      buildBundle({ ProviderWeeklyLabel: "本周" }, "chinese"),
    );
    const snapshot = provider(null, 31);
    snapshot.primary = rateWindow(31, { windowMinutes: 7 * 24 * 60 });
    snapshot.selectedMetric = snapshot.primary;

    renderCard(snapshot);

    expect(await screen.findByText("本周")).toBeInTheDocument();
  });

  it("notifies the tray panel after async local usage data loads", async () => {
    const onLayoutChange = vi.fn();

    renderCard(provider(null), { onLayoutChange });

    await waitFor(() => {
      expect(onLayoutChange).toHaveBeenCalled();
    });
  });

  it("shows the formatted predicted exhaustion time", async () => {
    const snapshot = provider(null, 40);
    snapshot.pace = {
      stage: "far_ahead",
      deltaPercent: 20,
      expectedUsedPercent: 20,
      actualUsedPercent: 40,
      etaSeconds: 90 * 60,
      willLastToReset: false,
    };

    const { container } = renderCard(snapshot);

    await waitFor(() => {
      expect(container.querySelector(".menu-card__pace-eta")).toHaveTextContent(
        "⚠ Runs out in 2h",
      );
    });
  });

  it("hides pace, budgets, and forecast text when Show pace is off", async () => {
    const resetAt = new Date(Date.now() + 6 * 24 * 60 * 60 * 1000);
    const snapshot = provider(null, 31);
    snapshot.pace = {
      stage: "far_ahead",
      deltaPercent: 20,
      expectedUsedPercent: 20,
      actualUsedPercent: 40,
      etaSeconds: 90 * 60,
      willLastToReset: false,
    };
    snapshot.primary = rateWindow(31, {
      windowMinutes: 7 * 24 * 60,
      resetsAt: resetAt.toISOString(),
    });

    const { container } = renderCard(snapshot, { showPace: false });

    expect(await screen.findByText("69% left")).toBeInTheDocument();
    expect(container.querySelector(".menu-card__pace")).not.toBeInTheDocument();
    expect(screen.queryByText("On-pace budget")).not.toBeInTheDocument();
    expect(container.querySelector(".menu-metric__forecast")).not.toBeInTheDocument();
  });

  it("renders local token and cost totals after chart data loads", async () => {
    const { container } = renderCard(provider(null));

    expect(await screen.findByText("30d cost")).toBeInTheDocument();
    expect(container.querySelector(".menu-card--with-details")).toBeInTheDocument();
    expect(container.querySelector(".menu-card--header-only")).not.toBeInTheDocument();
    expect(screen.getAllByText("$1.23").length).toBeGreaterThan(0);
    expect(screen.getByText("30d tokens")).toBeInTheDocument();
    expect(screen.getByText("584K")).toBeInTheDocument();
    expect(screen.getByText("Estimated from local logs")).toBeInTheDocument();
  });

  it("shows on-pace budgets and expands projection details", async () => {
    const onLayoutChange = vi.fn();
    const resetAt = new Date(
      Date.now() + 0.6 * 7 * 24 * 60 * 60 * 1000,
    );
    const snapshot = provider(null, 20);
    snapshot.primary = rateWindow(20, {
      reservePercent: 20,
      reserveWillLastToReset: true,
      windowMinutes: 7 * 24 * 60,
      resetsAt: resetAt.toISOString(),
    });

    renderCard(snapshot, { onLayoutChange });

    const toggle = await screen.findByRole("button", { name: /On-pace budget/ });
    expect(screen.getByText("now 20%")).toBeInTheDocument();
    expect(screen.getByText("1h 21%")).toBeInTheDocument();
    expect(screen.queryByRole("img", { name: /PaceChartAriaLabel/i })).not.toBeInTheDocument();

    fireEvent.click(toggle);

    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("img", { name: /PaceChartAriaLabel/i })).toBeInTheDocument();
    await waitFor(() => {
      expect(onLayoutChange).toHaveBeenCalled();
    });
  });

  it("shows on-pace budgets when timing exists without reserve metadata", async () => {
    const resetAt = new Date(Date.now() + 6 * 24 * 60 * 60 * 1000);
    const snapshot = provider(null, 31);
    snapshot.primary = rateWindow(31, {
      windowMinutes: 7 * 24 * 60,
      resetsAt: resetAt.toISOString(),
    });

    renderCard(snapshot);

    expect(
      await screen.findByRole("button", { name: /On-pace budget/ }),
    ).toBeInTheDocument();
      expect(screen.getByText("now 0%")).toBeInTheDocument();
      expect(screen.queryByText(/in reserve/)).not.toBeInTheDocument();
      expect(screen.queryByText("Lasts until reset")).not.toBeInTheDocument();
  });

  it("does not show pace budgets for a five-hour session window", async () => {
    const resetAt = new Date(Date.now() + 4 * 60 * 60 * 1000);
    const snapshot = provider(null, 31);
    snapshot.primary = rateWindow(31, {
      windowMinutes: 5 * 60,
      resetsAt: resetAt.toISOString(),
    });

    renderCard(snapshot);

    expect(await screen.findByText("69% left")).toBeInTheDocument();
    expect(screen.queryByText("On-pace budget")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("img", { name: /PaceChartAriaLabel/i }),
    ).not.toBeInTheDocument();
  });

  it("keeps the reserve row when timing data is incomplete", async () => {
    const snapshot = provider(null, 20);
    snapshot.primary = rateWindow(20, {
        reservePercent: 12,
        reserveWillLastToReset: true,
      });

    renderCard(snapshot);

    expect(await screen.findByText("12% in reserve")).toBeInTheDocument();
    expect(screen.queryByText("On-pace budget")).not.toBeInTheDocument();
  });


  it("shows spend used/limit plus balance secondary line", async () => {
    tauriMocks.getLocaleStrings.mockResolvedValue(
      buildBundle({
        DetailCostTitle: "Cost",
        DetailCostUsed: "Used",
        DetailCostBalance: "Balance",
        DetailCostRemaining: "Remaining",
      }),
    );
    const snapshot = provider(null, 20);
    snapshot.cost = {
      used: 12.5,
      limit: 100,
      remaining: 87.5,
      currencyCode: "USD",
      period: "Extra usage",
      resetsAt: null,
      formattedUsed: "$12.50",
      formattedLimit: "$100.00",
      balance: 25.5,
      formattedBalance: "$25.50",
    };

    renderCard(snapshot);

    expect(await screen.findByText(/Cost — Extra usage/)).toBeInTheDocument();
    expect(screen.getByText(/Used:\s*\$12\.50\s*\/\s*\$100\.00/)).toBeInTheDocument();
    expect(screen.getByText(/Balance:\s*\$25\.50/)).toBeInTheDocument();
  });

  it("renders balance-only cost as credits-style value", async () => {
    tauriMocks.getLocaleStrings.mockResolvedValue(
      buildBundle({
        CreditsLabel: "Credits",
        DetailCostTitle: "Cost",
        DetailCostUsed: "Used",
      }),
    );
    const snapshot = provider(null, 20);
    snapshot.cost = {
      used: 0,
      limit: null,
      remaining: null,
      currencyCode: "USD",
      period: "Extra usage",
      resetsAt: null,
      formattedUsed: "$0.00",
      formattedLimit: null,
      balance: 25.5,
      formattedBalance: "$25.50",
    };

    renderCard(snapshot);

    expect(await screen.findByText("Extra usage")).toBeInTheDocument();
    expect(screen.getByText("$25.50")).toBeInTheDocument();
    expect(screen.queryByText(/Used:/)).not.toBeInTheDocument();
  });

  it("localizes the relative updated-at time in Japanese without duplicated prefix", async () => {
    tauriMocks.getLocaleStrings.mockResolvedValue(
      buildBundle({
        UpdatedJustNow: "たった今",
        UpdatedMinutesAgo: "{}分前",
        UpdatedHoursAgo: "{}時間前",
        UpdatedDaysAgo: "{}日前",
      }),
    );

    const snapshot = provider(null, 20);
    snapshot.updatedAt = new Date(Date.now() - 3 * 60 * 1000).toISOString();
    renderCard(snapshot);

    expect(await screen.findByText("3分前")).toBeInTheDocument();
  });
});
