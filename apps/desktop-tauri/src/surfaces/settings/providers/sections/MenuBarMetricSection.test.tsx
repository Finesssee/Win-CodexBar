import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ProviderDetail } from "../../../../types/bridge";
import { MenuBarMetricSection } from "./MenuBarMetricSection";

function provider(extra = true): ProviderDetail {
  return {
    id: "copilot",
    displayName: "GitHub Copilot",
    enabled: true,
    email: null,
    plan: null,
    authType: null,
    sourceLabel: null,
    organization: null,
    lastUpdated: null,
    session: null,
    weekly: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: extra
      ? [{ id: "additional_budget", title: "Additional Budget", window: rateWindow(42) }]
      : [],
    cost: null,
    pace: null,
    lastError: null,
    errorState: null,
    dashboardUrl: null,
    statusPageUrl: null,
    buyCreditsUrl: null,
    hasSnapshot: true,
    cookieSource: null,
    region: null,
  };
}

function rateWindow(usedPercent: number) {
  return {
    usedPercent,
    remainingPercent: 100 - usedPercent,
    windowMinutes: null,
    resetsAt: null,
    resetDescription: null,
    isExhausted: false,
    reservePercent: null,
    reserveDescription: null,
  };
}

describe("MenuBarMetricSection", () => {
  it("offers Monthly for OpenCode Go only after a tertiary window is observed", () => {
    const base = provider(false);
    base.id = "opencodego";
    base.displayName = "OpenCode Go";
    base.tertiary = null;
    const { rerender } = render(
      <MenuBarMetricSection
        provider={base}
        providerMetrics={{}}
        disabled={false}
        t={(key) => key}
        onChange={vi.fn()}
      />,
    );

    expect(screen.queryByRole("option", { name: "DetailWindowTertiary" })).not.toBeInTheDocument();

    const observed = { ...base, tertiary: rateWindow(37) };
    rerender(
      <MenuBarMetricSection
        provider={observed}
        providerMetrics={{}}
        disabled={false}
        t={(key) => key}
        onChange={vi.fn()}
      />,
    );

    expect(screen.getByRole("option", { name: "DetailWindowTertiary" })).toBeInTheDocument();
  });
  it("offers extra usage when a provider has extra rate windows", () => {
    const onChange = vi.fn();
    render(
      <MenuBarMetricSection
        provider={provider()}
        providerMetrics={{}}
        disabled={false}
        t={(key) => key}
        onChange={onChange}
      />,
    );

    fireEvent.change(screen.getByRole("combobox"), { target: { value: "extraUsage" } });

    expect(screen.getByRole("option", { name: "ExtraUsage" })).toBeInTheDocument();
    expect(onChange).toHaveBeenCalledWith({
      providerMetrics: { copilot: "extraUsage" },
    });
  });
});
