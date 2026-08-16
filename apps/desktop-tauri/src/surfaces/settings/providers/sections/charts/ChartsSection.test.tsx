import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { ChartsSection } from "./ChartsSection";
import { getProviderChartData } from "../../../../../lib/tauri";
import type { ProviderChartData } from "../../../../../types/bridge";

vi.mock("../../../../../lib/tauri", () => ({
  getProviderChartData: vi.fn(),
  getSettingsSnapshot: vi.fn().mockResolvedValue({ enableAnimations: false }),
}));
vi.mock("../../../../../lib/providerCharts", () => ({
  providerSupportsChartData: () => true,
}));

const mockChart = vi.mocked(getProviderChartData);

function chartData(overrides: Partial<ProviderChartData>): ProviderChartData {
  return {
    providerId: "codex",
    costHistory: [{ date: "2026-08-16", value: 1.5 }],
    creditsHistory: [],
    usageBreakdown: [],
    localUsage: null,
    tokensHistory: [{ date: "2026-08-16", tokens: 9000 }],
    tokensIncomplete: false,
    ...overrides,
  };
}

describe("ChartsSection tokens mode (upstream 0.50.0 #2930)", () => {
  it("defaults Codex to the Tokens tab when exact token data exists", async () => {
    mockChart.mockResolvedValue(chartData({}));
    render(
      <ChartsSection
        providerId="codex"
        accountEmail={null}
        t={(key) => key}
      />,
    );
    await waitFor(() => {
      expect(screen.getByRole("tab", { selected: true }).textContent).toBe(
        "DetailChartTokens",
      );
    });
  });

  it("keeps Cost as the default for non-Codex providers", async () => {
    mockChart.mockResolvedValue(chartData({ providerId: "claude" }));
    render(
      <ChartsSection
        providerId="claude"
        accountEmail={null}
        t={(key) => key}
      />,
    );
    await waitFor(() => {
      expect(screen.getByRole("tab", { selected: true }).textContent).toBe(
        "DetailChartCost",
      );
    });
  });

  it("shows the Refreshing marker while local history backfill is incomplete", async () => {
    mockChart.mockResolvedValue(chartData({ tokensIncomplete: true }));
    render(
      <ChartsSection
        providerId="codex"
        accountEmail={null}
        t={(key) => key}
      />,
    );
    await waitFor(() => {
      expect(screen.getByText("DetailChartRefreshing")).toBeTruthy();
    });
  });

  it("hides the Tokens tab when no day carries token data", async () => {
    mockChart.mockResolvedValue(
      chartData({ tokensHistory: [{ date: "2026-08-16", tokens: 0 }] }),
    );
    render(
      <ChartsSection
        providerId="codex"
        accountEmail={null}
        t={(key) => key}
      />,
    );
    await waitFor(() => {
      expect(screen.getByRole("tab", { selected: true }).textContent).toBe(
        "DetailChartCost",
      );
    });
    expect(screen.queryByText("DetailChartTokens")).toBeNull();
  });
});
