import type {
  ProviderUsageSnapshot,
  RateWindowSnapshot,
} from "../types/bridge";

/** Selects the automatic window for surfaces rendering exactly one primary/secondary usage metric. */
export function selectSingleMetricUsageWindow(
  provider: Pick<ProviderUsageSnapshot, "primary" | "secondary">,
): RateWindowSnapshot {
  const { primary, secondary } = provider;
  return primary.isInformational && secondary && !secondary.isInformational
    ? secondary
    : primary;
}
