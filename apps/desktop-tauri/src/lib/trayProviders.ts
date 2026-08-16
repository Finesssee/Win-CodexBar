import type {
  ProviderCatalogEntry,
  ProviderUsageSnapshot,
  RateWindowSnapshot,
} from "../types/bridge";

export interface TrayProviderSlot {
  id: string;
  displayName: string;
}

const EMPTY_RATE_WINDOW: RateWindowSnapshot = {
  usedPercent: 0,
  remainingPercent: 100,
  windowMinutes: null,
  resetsAt: null,
  resetDescription: null,
  isExhausted: false,
  reservePercent: null,
  reserveDescription: null,
};

export function orderedEnabledProviderSlots(
  catalog: ProviderCatalogEntry[],
  enabledProviderIds: string[],
  snapshots: ProviderUsageSnapshot[],
  providerOrder: string[] = [],
): TrayProviderSlot[] {
  const enabled = new Set(enabledProviderIds);
  const catalogById = new Map(catalog.map((provider) => [provider.id, provider]));
  const snapshotNames = new Map(
    snapshots.map((provider) => [provider.providerId, provider.displayName]),
  );
  const slots: TrayProviderSlot[] = [];
  const seen = new Set<string>();
  const orderedIds = providerOrder.length > 0
    ? providerOrder
    : catalog.map((provider) => provider.id);

  for (const providerId of orderedIds) {
    if (!enabled.has(providerId)) continue;
    seen.add(providerId);
    slots.push({
      id: providerId,
      displayName:
        catalogById.get(providerId)?.displayName ?? snapshotNames.get(providerId) ?? providerId,
    });
  }

  for (const providerId of enabledProviderIds) {
    if (seen.has(providerId)) continue;
    slots.push({
      id: providerId,
      displayName: snapshotNames.get(providerId) ?? providerId,
    });
  }

  return slots;
}

export function providerPlaceholder(
  providerId: string,
  displayName: string,
): ProviderUsageSnapshot {
  return {
    providerId,
    displayName,
    primary: { ...EMPTY_RATE_WINDOW },
    selectedMetric: { ...EMPTY_RATE_WINDOW },
    primaryLabel: "Usage",
    secondary: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [],
    cost: null,
    planName: null,
    accountEmail: null,
    sourceLabel: "pending",
    updatedAt: new Date(0).toISOString(),
    error: "Loading provider data...",
    pace: null,
    accountOrganization: null,
    trayStatusLabel: null,
    fetchDurationMs: null,
  };
}

export function hydrateProviderSlots(
  slots: TrayProviderSlot[],
  providersById: Map<string, ProviderUsageSnapshot>,
): ProviderUsageSnapshot[] {
  return slots.map((slot) =>
    providersById.get(slot.id) ?? providerPlaceholder(slot.id, slot.displayName),
  );
}
