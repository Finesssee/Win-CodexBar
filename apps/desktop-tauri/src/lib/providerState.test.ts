import { describe, expect, it } from "vitest";
import { describeProviderState } from "./providerState";

describe("describeProviderState", () => {
  it.each([
    [null, "ready", "ProviderStatusOk"],
    ["authentication required", "needs-authentication", "ProviderIssueAuthRequired"],
    ["cookie session expired for https://private.example.test", "expired-session", "ProviderIssueSessionExpired"],
    ["legacy Gemini OAuth telemetry returned 403", "legacy-telemetry", "ProviderIssueLegacyTelemetry"],
    ["Antigravity local runtime is offline", "local-runtime-offline", "ProviderIssueLocalRuntimeOffline"],
    ["unexpected provider response", "unknown", "ProviderIssueUnknown"],
  ] as const)("maps %s to a safe %s descriptor", (error, kind, labelKey) => {
    expect(describeProviderState(error)).toEqual({
      kind,
      isProblem: kind !== "ready",
      labelKey,
    });
  });

  it("never returns raw error, cookie, or endpoint content", () => {
    const raw =
      "cookie=super-secret; request failed at https://private.example.test/v1";
    const descriptor = describeProviderState(raw);
    expect(JSON.stringify(descriptor)).not.toContain("super-secret");
    expect(JSON.stringify(descriptor)).not.toContain("private.example.test");
    expect(JSON.stringify(descriptor)).not.toContain("cookie=");
  });
});
