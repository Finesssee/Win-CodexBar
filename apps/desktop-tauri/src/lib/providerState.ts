import type { LocaleKey } from "../i18n/keys";

/**
 * A presentation-safe summary of a provider refresh result.
 *
 * Provider errors can contain account, host, path, or credential details.
 * Keep those details out of compact and settings-facing surfaces. The raw
 * value stays in the backend diagnostic path; this helper exposes only a
 * stable category that is safe to render.
 */
export type ProviderStateKind =
  | "ready"
  | "needs-authentication"
  | "expired-session"
  | "legacy-telemetry"
  | "local-runtime-offline"
  | "unknown";

export interface ProviderStateDescriptor {
  kind: ProviderStateKind;
  isProblem: boolean;
  labelKey: LocaleKey;
}

const AUTH_PATTERN =
  /\bauth(?:entication|orization)?\b|\bsign[ -]?in\b|\blog[ -]?in\b|\bcredential(?:s)?\b|\bcookie(?:s)?\b|\boauth\b|\bunauthori[sz]ed\b|\bforbidden\b|\bpermission denied\b/i;
const EXPIRED_SESSION_PATTERN =
  /\b(?:session|token|cookie|credential|oauth)\b[^\n]{0,48}\b(?:expired|invalid|revoked)\b|\b(?:expired|invalid|revoked)\b[^\n]{0,48}\b(?:session|token|cookie|credential|oauth)\b/i;
const LEGACY_TELEMETRY_PATTERN =
  /\blegacy\b|\btelemetry\b|\bdeprecated\b|\bsource mode\b[^\n]{0,48}\bnot supported\b/i;
const LOCAL_RUNTIME_PATTERN =
  /\b(?:local|runtime|daemon|service|app|cli|language server)\b[^\n]{0,48}\b(?:offline|not running|unavailable|not found)\b|\bconnection refused\b|\boffline\b/i;

/**
 * Categorize a raw refresh error without returning any part of that error.
 * Ordering matters: an explicitly expired credential is more useful than the
 * broader authentication category, and a legacy telemetry source is distinct
 * from a local runtime that is simply not running.
 */
export function describeProviderState(error: string | null | undefined): ProviderStateDescriptor {
  if (!error?.trim()) {
    return { kind: "ready", isProblem: false, labelKey: "ProviderStatusOk" };
  }

  if (EXPIRED_SESSION_PATTERN.test(error)) {
    return {
      kind: "expired-session",
      isProblem: true,
      labelKey: "ProviderIssueSessionExpired",
    };
  }
  if (LEGACY_TELEMETRY_PATTERN.test(error)) {
    return {
      kind: "legacy-telemetry",
      isProblem: true,
      labelKey: "ProviderIssueLegacyTelemetry",
    };
  }
  if (AUTH_PATTERN.test(error)) {
    return {
      kind: "needs-authentication",
      isProblem: true,
      labelKey: "ProviderIssueAuthRequired",
    };
  }
  if (LOCAL_RUNTIME_PATTERN.test(error)) {
    return {
      kind: "local-runtime-offline",
      isProblem: true,
      labelKey: "ProviderIssueLocalRuntimeOffline",
    };
  }
  return {
    kind: "unknown",
    isProblem: true,
    labelKey: "ProviderIssueUnknown",
  };
}
