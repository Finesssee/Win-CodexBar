import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("../../../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key }),
}));

// Mock Tauri invoke for get_available_languages
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue([
    { value: "english", display: "English" },
    { value: "chinese", display: "中文" },
    { value: "chinesetraditional", display: "繁體中文" },
    { value: "japanese", display: "日本語" },
    { value: "korean", display: "한국어" },
    { value: "spanish", display: "Español" },
    { value: "russian", display: "Русский" },
    { value: "turkish", display: "Türkçe" },
  ]),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

import GeneralTab from "./GeneralTab";
import type { SettingsSnapshot } from "../../../types/bridge";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

const settings: SettingsSnapshot = {
  enabledProviders: [],
  refreshIntervalSecs: 300,
    adaptiveRefresh: false,
  refreshAllProvidersOnMenuOpen: false,
  lowPowerMode: false,
  startAtLogin: false,
  startMinimized: false,
  showNotifications: true,
  soundEnabled: true,
  notificationSoundTheme: "windows",
  notificationSoundPaths: {
    predictiveWarning: null,
    highUsage: null,
    criticalUsage: null,
    exhausted: null,
    statusIssue: null,
    sessionDepleted: null,
    sessionRestored: null,
  },
  highUsageThreshold: 70,
  criticalUsageThreshold: 90,
  predictivePaceWarningEnabled: false,
  trayIconMode: "single",
  switcherShowsIcons: true,
  menuBarShowsHighestUsage: true,
  menuBarShowsPercent: true,
  showAsUsed: false,
  showAllTokenAccountsInMenu: true,
  enableAnimations: true,
  resetTimeRelative: true,
  menuBarDisplayMode: "compact",
  windowScalePercent: 125,
  trayScalePercent: 100,
  powertoysStatusPipeEnabled: false,
  hidePersonalInfo: false,
  autoDownloadUpdates: false,
  installUpdatesOnQuit: false,
  globalShortcut: "",
  codexCustomSessionsDirs: [],
  updateChannel: "stable",
  uiLanguage: "english",
  theme: "dark",
  claudeAvoidKeychainPrompts: true,
  codexSparkUsageVisible: true,
  disableKeychainAccess: false,
  providerMetrics: {},
  floatBarEnabled: false,
  floatBarOpacity: 0.9,
  floatBarScale: 100,
  floatBarOrientation: "horizontal",
  floatBarStyle: "floating",
  floatBarClickThrough: false,
  floatBarProviderIds: [],
  floatBarDarkText: false,
  floatBarShowResetInline: false,
  floatBarShowCost: false,
  claudeDailyRoutinesUsageVisible: true,
  alibabaTokenPlanRegion: "cn",
  weeklyProgressWorkDays: null,
  showResetWhenExhausted: false,
};

describe("GeneralTab language picker", () => {
  it("renders all supported language options", () => {
    render(<GeneralTab settings={settings} set={vi.fn()} saving={false} />);

    const select = screen.getByDisplayValue("English");
    expect(select).toBeInTheDocument();

    const options = select.querySelectorAll("option");
    expect(options.length).toBeGreaterThanOrEqual(8);
  });

  it("includes spanish as a selectable option", () => {
    render(<GeneralTab settings={settings} set={vi.fn()} saving={false} />);

    expect(
      screen.getByText("Español"),
    ).toBeInTheDocument();
  });

  it("includes russian as a selectable option", () => {
    render(<GeneralTab settings={settings} set={vi.fn()} saving={false} />);

    expect(screen.getByText("Русский")).toBeInTheDocument();
  });

  it("includes turkish as a selectable option", () => {
    render(<GeneralTab settings={settings} set={vi.fn()} saving={false} />);

    expect(screen.getByText("Türkçe")).toBeInTheDocument();
  });

  it("includes korean as a selectable option", () => {
    render(<GeneralTab settings={settings} set={vi.fn()} saving={false} />);

    expect(
      screen.getByText("한국어"),
    ).toBeInTheDocument();
  });

  it("includes Traditional Chinese as a selectable option", () => {
    render(<GeneralTab settings={settings} set={vi.fn()} saving={false} />);

    expect(screen.getByText("繁體中文")).toBeInTheDocument();
  });

  it("updates the predictive pace warning preference", () => {
    const set = vi.fn();
    render(
      <GeneralTab
        mode="notifications"
        settings={settings}
        set={set}
        saving={false}
      />,
    );

    fireEvent.click(screen.getByRole("checkbox", { name: "PredictivePaceWarnings" }));

    expect(set).toHaveBeenCalledWith({ predictivePaceWarningEnabled: true });
  });

  it("updates the low power mode preference", () => {
    const set = vi.fn();
    render(<GeneralTab settings={settings} set={set} saving={false} />);

    fireEvent.click(screen.getByRole("checkbox", { name: "LowPowerMode" }));

    expect(set).toHaveBeenCalledWith({ lowPowerMode: true });
  });

  it("updates the default notification sound set", () => {
    const set = vi.fn();
    render(
      <GeneralTab mode="notifications" settings={settings} set={set} saving={false} />,
    );

    const select = screen.getByRole("combobox", { name: "NotificationSoundTheme" });
    expect(select.querySelectorAll("option")).toHaveLength(2);
    expect(select).toHaveStyle({ width: "180px" });
    fireEvent.change(select, {
      target: { value: "codexBar" },
    });

    expect(set).toHaveBeenCalledWith({ notificationSoundTheme: "codexBar" });
  });

  it("renders and previews all seven notification events", () => {
    render(
      <GeneralTab mode="notifications" settings={settings} set={vi.fn()} saving={false} />,
    );

    const previewButtons = screen.getAllByRole("button", {
      name: /NotificationTestSound$/,
    });
    expect(previewButtons).toHaveLength(7);

    fireEvent.click(
      screen.getByRole("button", {
        name: "NotificationSoundEventSessionRestored: NotificationTestSound",
      }),
    );
    expect(invoke).toHaveBeenCalledWith("play_notification_sound", {
      event: "sessionRestored",
    });
  });

  it("assigns and clears a custom WAV for one notification", async () => {
    const set = vi.fn();
    vi.mocked(open).mockResolvedValue("C:\\sounds\\high-usage.wav");
    const { rerender } = render(
      <GeneralTab mode="notifications" settings={settings} set={set} saving={false} />,
    );

    fireEvent.click(
      screen.getByRole("button", {
        name: "NotificationSoundEventHighUsage: NotificationSoundChooseFile",
      }),
    );
    await waitFor(() =>
      expect(set).toHaveBeenCalledWith({
        notificationSoundPaths: {
          ...settings.notificationSoundPaths,
          highUsage: "C:\\sounds\\high-usage.wav",
        },
      }),
    );

    rerender(
      <GeneralTab
        mode="notifications"
        settings={{
          ...settings,
          notificationSoundPaths: {
            ...settings.notificationSoundPaths,
            highUsage: "C:\\sounds\\high-usage.wav",
          },
        }}
        set={set}
        saving={false}
      />,
    );
    expect(
      screen.getByRole("button", {
        name: "NotificationSoundEventHighUsage: high-usage.wav, NotificationSoundChooseFile",
      }),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", {
        name: "NotificationSoundEventHighUsage: NotificationSoundClearFile",
      }),
    );
    expect(set).toHaveBeenLastCalledWith({
      notificationSoundPaths: settings.notificationSoundPaths,
    });
  });

  it("reenables sound previews immediately when playback fails", async () => {
    render(
      <GeneralTab mode="notifications" settings={settings} set={vi.fn()} saving={false} />,
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("get_available_languages"),
    );
    vi.mocked(invoke).mockRejectedValueOnce(new Error("playback failed"));

    const preview = screen.getByRole("button", {
      name: "NotificationSoundEventCriticalUsage: NotificationTestSound",
    });
    fireEvent.click(preview);

    expect(await screen.findByRole("alert")).toHaveTextContent("playback failed");
    expect(preview).toBeEnabled();
  });

  it("saves a window override on blur and clears it to resume inheritance", () => {
    const set = vi.fn();
    const { rerender } = render(
      <GeneralTab mode="notifications" settings={settings} set={set} saving={false} />,
    );
    const input = screen.getByRole("spinbutton", {
      name: "ProviderNameCodex · ProviderSession HighUsageAlert",
    });

    fireEvent.change(input, { target: { value: "80" } });
    fireEvent.blur(input);
    expect(set).toHaveBeenLastCalledWith({
      providerUsageThresholds: { "codex:session": { high: 80 } },
    });

    rerender(
      <GeneralTab
        mode="notifications"
        settings={{
          ...settings,
          providerUsageThresholds: { "codex:session": { high: 80 } },
        }}
        set={set}
        saving={false}
      />,
    );
    const saved = screen.getByRole("spinbutton", {
      name: "ProviderNameCodex · ProviderSession HighUsageAlert",
    });
    fireEvent.change(saved, { target: { value: "" } });
    fireEvent.blur(saved);
    expect(set).toHaveBeenLastCalledWith({ providerUsageThresholds: {} });
  });
});
