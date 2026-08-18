import { describe, it, expect } from "vitest";
import {
  providerCostColor,
  providerCreditsColor,
  serviceColorVar,
} from "./chartPalette";

describe("chartPalette.providerColor", () => {
  it("returns a CSS var() expression referencing a provider token for known ids", () => {
    expect(providerCostColor("claude")).toBe(
      "var(--chart-claude, var(--provider-accent, var(--chart-cost)))",
    );
    expect(providerCreditsColor("codex")).toBe(
      "var(--chart-codex, var(--provider-accent, var(--chart-credits)))",
    );
  });

  it("is case-insensitive and handles spaced aliases", () => {
    expect(providerCostColor("CURSOR")).toBe(
      "var(--chart-cursor, var(--provider-accent, var(--chart-cost)))",
    );
    expect(providerCostColor("Kimi K2")).toBe(
      "var(--chart-kimik2, var(--provider-accent, var(--chart-cost)))",
    );
    expect(providerCostColor("Vertex AI")).toBe(
      "var(--chart-vertexai, var(--provider-accent, var(--chart-cost)))",
    );
  });

  it("falls back to the generic cost/credits token for unknown providers", () => {
    expect(providerCostColor("unknown-provider-xyz")).toBe(
      "var(--provider-accent, var(--chart-cost))",
    );
    expect(providerCreditsColor("another-ghost")).toBe(
      "var(--provider-accent, var(--chart-credits))",
    );
  });
});

describe("chartPalette.serviceColorVar", () => {
  it("routes named service kinds to dedicated tokens", () => {
    expect(serviceColorVar("cli", ["cli"])).toBe("var(--chart-service-cli)");
    expect(serviceColorVar("github code review", ["github code review"])).toBe(
      "var(--chart-service-review)",
    );
    expect(serviceColorVar("api-calls", ["api-calls"])).toBe(
      "var(--chart-service-api)",
    );
  });

  it("assigns deterministic palette slots (1..5) to unknown services", () => {
    const ordered = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"];
    expect(serviceColorVar("alpha", ordered)).toBe("var(--chart-service-1)");
    expect(serviceColorVar("epsilon", ordered)).toBe("var(--chart-service-5)");
    // slot wraps with modulo 5 → "zeta" is index 5 → slot 1
    expect(serviceColorVar("zeta", ordered)).toBe("var(--chart-service-1)");
  });
});
