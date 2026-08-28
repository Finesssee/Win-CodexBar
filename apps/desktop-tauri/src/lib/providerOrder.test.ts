import { describe, expect, it } from "vitest";
import { orderProviderSnapshots } from "./providerOrder";
import type { ProviderCatalogEntry, ProviderUsageSnapshot } from "../types/bridge";

const catalog: ProviderCatalogEntry[] = [
  { id: "codex", displayName: "Codex", cookieDomain: null },
  { id: "claude", displayName: "Claude", cookieDomain: null },
  { id: "gemini", displayName: "Gemini", cookieDomain: null },
];

function snapshot(providerId: string, displayName: string): ProviderUsageSnapshot {
  return {
    providerId,
    displayName,
    primary: {
      usedPercent: 0,
      remainingPercent: 100,
      windowMinutes: null,
      resetsAt: null,
      resetDescription: null,
      isExhausted: false,
      reservePercent: null,
      reserveDescription: null,
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
    },
    secondary: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    cost: null,
    planName: null,
    accountEmail: null,
    sourceLabel: "test",
    updatedAt: "2026-01-01T00:00:00Z",
    error: null,
    errorState: "ready",
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
  };
}

describe("orderProviderSnapshots", () => {
  it("uses persisted provider order before catalog order", () => {
    const ordered = orderProviderSnapshots(
      [
        snapshot("codex", "Codex"),
        snapshot("claude", "Claude"),
        snapshot("gemini", "Gemini"),
      ],
      catalog,
      ["codex", "claude", "gemini"],
      ["gemini", "claude", "codex"],
    );

    expect(ordered.map((provider) => provider.providerId)).toEqual([
      "gemini",
      "claude",
      "codex",
    ]);
  });
});
