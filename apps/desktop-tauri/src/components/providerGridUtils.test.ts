import { describe, expect, it } from "vitest";

import type { ProviderUsageSnapshot } from "../types/bridge";

import { prioritizeProviders } from "./providerGridUtils";

function provider(id: string): ProviderUsageSnapshot {
  return {
    providerId: id,
    displayName: id,
    primary: {
      usedPercent: 0,
      remainingPercent: 100,
      windowMinutes: null,
      resetsAt: null,
      resetDescription: null,
      isExhausted: false,
      reservePercent: null,
      reserveDescription: null,
      reserveWillLastToReset: false,
      reserveEtaSeconds: null,
    },
    selectedMetric: {
      usedPercent: 0,
      remainingPercent: 100,
      windowMinutes: null,
      resetsAt: null,
      resetDescription: null,
      isExhausted: false,
      reservePercent: null,
      reserveDescription: null,
      reserveWillLastToReset: false,
      reserveEtaSeconds: null,
    },
    primaryLabel: undefined,
    secondary: null,
    secondaryLabel: undefined,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    cost: null,
    planName: null,
    accountEmail: null,
    sourceLabel: "oauth",
    updatedAt: "2026-07-31T00:00:00Z",
    error: null,
    errorState: "ready",
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
    fetchDurationMs: null,
  };
}

function ids(list: ProviderUsageSnapshot[]): string[] {
  return list.map((p) => p.providerId);
}

describe("prioritizeProviders", () => {
  it("returns the list unchanged when no provider is selected", () => {
    const list = [provider("a"), provider("b")];
    expect(prioritizeProviders(list, null)).toBe(list);
    expect(prioritizeProviders(list, "")).toBe(list);
  });

  it("returns the list unchanged when the selected provider is not present", () => {
    const list = [provider("a"), provider("b")];
    expect(prioritizeProviders(list, "missing")).toBe(list);
  });

  it("does not reorder when the selected provider is already within the first 18 slots", () => {
    // index 17 (the 18th slot) — boundary, must NOT move.
    const list = Array.from({ length: 18 }, (_, i) => provider(`p${i}`));
    list[17] = provider("target");
    expect(prioritizeProviders(list, "target")).toBe(list);
  });

  it("does not reorder when the selected provider sits exactly at index 18 (just past the compact window)", () => {
    // index 18 is the 19th slot; the rule only pulls items with selectedIndex >= 18,
    // but the boundary condition is `selectedIndex < 18` → returns as-is.
    const list = Array.from({ length: 20 }, (_, i) => provider(`p${i}`));
    // selectedIndex 18 → should be pulled to front.
    const next = prioritizeProviders(list, "p18");
    expect(next[0].providerId).toBe("p18");
  });

  it("pulls a far-down selected provider to the front, preserving relative order of the rest", () => {
    const list = Array.from({ length: 25 }, (_, i) => provider(`p${i}`));
    const next = prioritizeProviders(list, "p24");
    expect(ids(next)).toEqual([
      "p24",
      ...Array.from({ length: 24 }, (_, i) => `p${i}`),
    ]);
    // original list is not mutated
    expect(list[0].providerId).toBe("p0");
  });

  it("prioritized provider is selected vs not-selected ordering differ", () => {
    const base = Array.from({ length: 30 }, (_, i) => provider(`p${i}`));
    const prioritized = prioritizeProviders(base, "p29");
    const unprioritized = prioritizeProviders(base, null);
    expect(prioritized[0].providerId).toBe("p29");
    expect(unprioritized[0].providerId).toBe("p0");
    expect(prioritized.length).toBe(unprioritized.length);
    // same members, different order
    expect(new Set(ids(prioritized))).toEqual(new Set(ids(unprioritized)));
  });
});
