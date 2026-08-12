import { describe, expect, it } from "vitest";
import type { RateWindowSnapshot } from "../types/bridge";
import { selectSingleMetricUsageWindow } from "./usageWindows";

function rateWindow(
  overrides: Partial<RateWindowSnapshot> = {},
): RateWindowSnapshot {
  return {
    usedPercent: 0,
    remainingPercent: 100,
    windowMinutes: null,
    resetsAt: null,
    resetDescription: null,
    isExhausted: false,
    reservePercent: null,
    reserveDescription: null,
    ...overrides,
  };
}

describe("selectSingleMetricUsageWindow", () => {
  it("returns a normal primary when a secondary exists", () => {
    const primary = rateWindow();
    const secondary = rateWindow({ isInformational: true });

    expect(selectSingleMetricUsageWindow({ primary, secondary })).toBe(primary);
  });

  it("returns a real secondary when the primary is informational", () => {
    const primary = rateWindow({ isInformational: true });
    const secondary = rateWindow();

    expect(selectSingleMetricUsageWindow({ primary, secondary })).toBe(secondary);
  });

  it("returns an informational primary when the secondary is null", () => {
    const primary = rateWindow({ isInformational: true });

    expect(selectSingleMetricUsageWindow({ primary, secondary: null })).toBe(primary);
  });

  it("returns an informational primary when the secondary is informational", () => {
    const primary = rateWindow({ isInformational: true });
    const secondary = rateWindow({ isInformational: true });

    expect(selectSingleMetricUsageWindow({ primary, secondary })).toBe(primary);
  });
});
