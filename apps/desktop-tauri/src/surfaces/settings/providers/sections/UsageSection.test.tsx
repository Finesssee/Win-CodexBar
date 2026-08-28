import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LocaleProvider } from "../../../../i18n/LocaleProvider";
import { buildBundle } from "../../../../test/localeHarness";
import type { ProviderDetail } from "../../../../types/bridge";
import { UsageSection } from "./UsageSection";

const tauriMocks = vi.hoisted(() => ({
  getLocaleStrings: vi.fn(),
  setUiLanguage: vi.fn(),
}));

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(),
}));

vi.mock("../../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../../lib/tauri")>()),
  ...tauriMocks,
}));
vi.mock("@tauri-apps/api/event", () => eventMocks);

function rateWindow(usedPercent: number) {
  return {
    usedPercent,
    remainingPercent: 100 - usedPercent,
    windowMinutes: null,
    resetsAt: null,
    resetDescription: null,
    isExhausted: false,
    reservePercent: null,
    reserveDescription: null,
  };
}

function provider(): ProviderDetail {
  return {
    id: "copilot",
    displayName: "GitHub Copilot",
    enabled: true,
    email: null,
    plan: null,
    authType: null,
    sourceLabel: null,
    organization: null,
    lastUpdated: null,
    session: rateWindow(20),
    weekly: null,
    modelSpecific: null,
    tertiary: null,
    extraRateWindows: [
      { id: "additional_budget", title: "Additional Budget", window: rateWindow(42) },
    ],
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
  };
}

describe("UsageSection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.getLocaleStrings.mockResolvedValue(buildBundle());
    eventMocks.listen.mockResolvedValue(() => {});
  });

  it("renders extra Copilot budget windows in settings", async () => {
    render(
      <LocaleProvider>
        <UsageSection provider={provider()} resetTimeRelative={true} t={(key) => key} />
      </LocaleProvider>,
    );

    expect(await screen.findByText("Additional Budget")).toBeInTheDocument();
    expect(screen.getByText("42%")).toBeInTheDocument();
  });

  it("marks an unavailable session without rendering a quota bar", async () => {
    const detail = provider();
    detail.session = {
      ...rateWindow(0),
      isInformational: true,
      resetDescription: "No active 5h session",
    };

    render(
      <LocaleProvider>
        <UsageSection provider={detail} resetTimeRelative={true} t={(key) => key} />
      </LocaleProvider>,
    );

    const label = await screen.findByText("ProviderSessionLabel");
    expect(label.parentElement).toHaveTextContent("No active 5h session");
    expect(label.parentElement?.querySelector(".provider-usage-bar__track")).toBeNull();
  });
});
