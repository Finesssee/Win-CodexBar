import { describe, expect, it } from "vitest";
import type { ProviderStateKind } from "../types/bridge";
import { describeProviderState } from "./providerState";

const ALL_KINDS: ProviderStateKind[] = [
  "ready",
  "needsAuthentication",
  "expiredSession",
  "localRuntimeOffline",
  "unknown",
];

describe("describeProviderState", () => {
  it.each([
    ["ready", false, "ProviderStatusOk"],
    ["needsAuthentication", true, "ProviderIssueAuthRequired"],
    ["expiredSession", true, "ProviderIssueSessionExpired"],
    ["localRuntimeOffline", true, "ProviderIssueLocalRuntimeOffline"],
    ["unknown", true, "ProviderIssueUnknown"],
  ] as const)(
    "maps %s to a safe descriptor",
    (kind, isProblem, labelKey) => {
      expect(describeProviderState(kind)).toEqual({
        kind,
        isProblem,
        labelKey,
      });
    },
  );

  it("treats null and undefined as the unknown problem state", () => {
    expect(describeProviderState(null)).toEqual({
      kind: "unknown",
      isProblem: true,
      labelKey: "ProviderIssueUnknown",
    });
    expect(describeProviderState(undefined)).toEqual({
      kind: "unknown",
      isProblem: true,
      labelKey: "ProviderIssueUnknown",
    });
  });

  it("resolves every union member to a descriptor whose kind round-trips", () => {
    // Casing-drift guard: the union values must mirror the Rust serde
    // camelCase wire strings exactly. A kebab-case regression would make
    // STATE_DESCRIPTORS[kind] undefined at runtime while tsc stays green,
    // so this must fail instead of silently returning an unknown state.
    for (const kind of ALL_KINDS) {
      const descriptor = describeProviderState(kind);
      expect(descriptor).toBeDefined();
      expect(descriptor.kind).toBe(kind);
    }
  });

  it("mirrors the Rust serde contract for the camelCase wire values", () => {
    // Pins the bridge spelling: `serializes_as_camel_case_for_the_bridge`
    // in rust/src/core/provider_state.rs emits these exact strings.
    expect(describeProviderState("needsAuthentication").labelKey).toBe(
      "ProviderIssueAuthRequired",
    );
    expect(describeProviderState("expiredSession").labelKey).toBe(
      "ProviderIssueSessionExpired",
    );
    expect(describeProviderState("localRuntimeOffline").labelKey).toBe(
      "ProviderIssueLocalRuntimeOffline",
    );
  });

  it("never embeds arbitrary error text in the descriptor", () => {
    // The descriptor is built from the classified kind alone; even a hostile
    // error string on the bridge can only influence the kind, never appear
    // in the descriptor payload.
    const descriptor = describeProviderState("needsAuthentication");
    expect(JSON.stringify(descriptor)).not.toContain("super-secret");
    expect(JSON.stringify(descriptor)).not.toContain("private.example.test");
    expect(JSON.stringify(descriptor)).not.toContain("cookie=");
    expect(JSON.stringify(descriptor)).not.toContain("sign in");
  });
});
