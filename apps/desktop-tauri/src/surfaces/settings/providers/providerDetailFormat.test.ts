import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ProviderDetail } from "../../../types/bridge";
import type { LocaleKey } from "../../../i18n/keys";

import { buildSubtitle, relativeAgo } from "./providerDetailFormat";

/** Identity translator — returns the key verbatim so assertions are stable. */
const t = (k: LocaleKey) => k as string;

function baseDetail(over: Partial<ProviderDetail> = {}): ProviderDetail {
  return {
    id: "claude",
    displayName: "Claude",
    enabled: true,
    email: "team@example.com",
    plan: "Pro",
    authType: "oauth",
    sourceLabel: "oauth",
    organization: null,
    lastUpdated: null,
    session: null,
    weekly: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
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
    ...over,
  };
}

describe("buildSubtitle", () => {
  it("returns an empty string when sourceLabel and lastUpdated are both null", () => {
    expect(buildSubtitle(baseDetail({ sourceLabel: null, lastUpdated: null }), t)).toBe("");
  });

  it("emits only the not-fetched marker when there is no snapshot and no source", () => {
    const detail = baseDetail({
      sourceLabel: null,
      lastUpdated: null,
      hasSnapshot: false,
    });
    expect(buildSubtitle(detail, t)).toBe("ProviderUsageNotFetchedYet");
  });

  it("shows only sourceLabel when a snapshot exists but no lastUpdated", () => {
    expect(buildSubtitle(baseDetail({ hasSnapshot: true }), t)).toBe("oauth");
  });

  it("joins sourceLabel with the updated prefix when lastUpdated is present", () => {
    const detail = baseDetail({ lastUpdated: "2026-07-01T00:00:00Z" });
    // Pin now far ahead of the timestamp so relativeAgo returns a deterministic "d" bucket.
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-31T00:00:00Z"));
    try {
      expect(buildSubtitle(detail, t)).toBe("oauth · DetailUpdatedPrefix 30d");
    } finally {
      vi.useRealTimers();
    }
  });

  it("still shows sourceLabel when plan identity is missing (plan does not drive subtitle)", () => {
    const detail = baseDetail({ plan: null, lastUpdated: null, hasSnapshot: true });
    expect(buildSubtitle(detail, t)).toBe("oauth");
  });
});

describe("relativeAgo", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-31T12:00:00Z"));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("returns null for an unparseable timestamp", () => {
    expect(relativeAgo("not-a-date")).toBeNull();
  });

  it("returns null for an empty string", () => {
    expect(relativeAgo("")).toBeNull();
  });

  it("uses seconds bucket for 0ms through the 59s boundary", () => {
    expect(relativeAgo(new Date("2026-07-31T12:00:00Z").toISOString())).toBe("0s");
    expect(relativeAgo(new Date("2026-07-31T11:59:31Z").toISOString())).toBe("29s");
    expect(relativeAgo(new Date("2026-07-31T11:59:01Z").toISOString())).toBe("59s");
  });

  it("uses minutes bucket at the 60s boundary and up to 59m", () => {
    // exactly 60s rounds to 1m
    expect(relativeAgo(new Date("2026-07-31T11:59:00Z").toISOString())).toBe("1m");
    expect(relativeAgo(new Date("2026-07-31T11:01:00Z").toISOString())).toBe("59m");
  });

  it("uses hours bucket at 60m and up to 23h", () => {
    expect(relativeAgo(new Date("2026-07-31T11:00:00Z").toISOString())).toBe("1h");
    expect(relativeAgo(new Date("2026-07-30T13:00:00Z").toISOString())).toBe("23h");
  });

  it("uses days bucket at the 24h boundary", () => {
    expect(relativeAgo(new Date("2026-07-30T12:00:00Z").toISOString())).toBe("1d");
    expect(relativeAgo(new Date("2026-07-01T12:00:00Z").toISOString())).toBe("30d");
  });

  it("is symmetric for future timestamps (uses absolute diff)", () => {
    // 90s in the future
    expect(relativeAgo(new Date("2026-07-31T12:01:30Z").toISOString())).toBe("2m");
  });
});
