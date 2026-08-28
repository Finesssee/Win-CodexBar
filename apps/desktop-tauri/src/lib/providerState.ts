import type { LocaleKey } from "../i18n/keys";
import type { ProviderStateKind as BridgeProviderStateKind } from "../types/bridge";

/**
 * Backend-classified provider availability state.
 *
 * Classification happens in Rust from the typed `ProviderError` and travels
 * on the snapshot/detail bridge as `errorState` (serde camelCase:
 * `"needsAuthentication"`, `"expiredSession"`, `"localRuntimeOffline"`).
 * The union values here must mirror those wire strings exactly — this
 * module only maps the kind to a locale key, never the raw error text.
 */
export type ProviderStateKind =
  | "ready"
  | "needsAuthentication"
  | "expiredSession"
  | "localRuntimeOffline"
  | "unknown";

export interface ProviderStateDescriptor {
  kind: ProviderStateKind;
  isProblem: boolean;
  labelKey: LocaleKey;
}

const STATE_DESCRIPTORS: Record<ProviderStateKind, ProviderStateDescriptor> = {
  ready: { kind: "ready", isProblem: false, labelKey: "ProviderStatusOk" },
  needsAuthentication: {
    kind: "needsAuthentication",
    isProblem: true,
    labelKey: "ProviderIssueAuthRequired",
  },
  expiredSession: {
    kind: "expiredSession",
    isProblem: true,
    labelKey: "ProviderIssueSessionExpired",
  },
  localRuntimeOffline: {
    kind: "localRuntimeOffline",
    isProblem: true,
    labelKey: "ProviderIssueLocalRuntimeOffline",
  },
  unknown: {
    kind: "unknown",
    isProblem: true,
    labelKey: "ProviderIssueUnknown",
  },
};

/**
 * Map a backend-classified state to its presentation descriptor. Missing
 * values (absent bridge field, legacy snapshot) fall back to the safe
 * `unknown` problem state.
 */
export function describeProviderState(
  kind: BridgeProviderStateKind | null | undefined,
): ProviderStateDescriptor {
  if (!kind) {
    return STATE_DESCRIPTORS.unknown;
  }
  return STATE_DESCRIPTORS[kind];
}
