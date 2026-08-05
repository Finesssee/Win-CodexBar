import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  CodexAccount,
  CodexAccountsStateBridge,
  CodexAccountUsageSnapshot,
  CodexSwitchResult,
} from "../../../../../types/bridge";

const tauriMocks = vi.hoisted(() => ({
  getCodexAccountsState: vi.fn(),
  codexAccountAdd: vi.fn(),
  codexAccountFetch: vi.fn(),
  codexAccountRemove: vi.fn(),
  codexAccountSwitch: vi.fn(),
  codexAccountRestartDesktop: vi.fn(),
}));

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("../../../../../lib/tauri", () => tauriMocks);
vi.mock("@tauri-apps/api/event", () => eventMocks);

import { CodexAccountsSection } from "./CodexAccountsSection";

const t = (key: string) => key;

function account(id: string, extra: Partial<CodexAccount> = {}): CodexAccount {
  return {
    id,
    nickname: null,
    emailHint: `user-${id}@example.com`,
    authSubject: null,
    providerAccountId: null,
    codexHomePath: `C:/fake/${id}`,
    source: "managedByApp",
    createdAt: "2024-01-01T00:00:00Z",
    updatedAt: "2024-01-01T00:00:00Z",
    lastAuthenticatedAt: null,
    ...extra,
  };
}

function snapshot(usedPercent: number, plan = "free"): CodexAccountUsageSnapshot {
  return {
    email: "user@example.com",
    providerAccountId: null,
    plan,
    allowed: true,
    limitReached: false,
    primaryWindow: { usedPercent, resetAt: null, limitWindowSeconds: 3600 },
    secondaryWindow: null,
    credits: null,
    updatedAt: "2024-01-01T00:00:00Z",
  };
}

describe("CodexAccountsSection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders nothing before the store loads, then lists accounts", async () => {
    tauriMocks.getCodexAccountsState.mockResolvedValue(
      { accounts: [account("1"), account("2", { source: "ambient" })], snapshots: {} } as CodexAccountsStateBridge,
    );
    const { container } = render(<CodexAccountsSection t={t} />);
    expect(container.querySelector(".codex-accounts")).toBeNull();

    await waitFor(() => {
      expect(screen.getByText("user-1@example.com")).toBeDefined();
    });
    expect(screen.getByText("user-2@example.com")).toBeDefined();
    expect(screen.getByText("CodexAccountsSourceManaged")).toBeDefined();
    expect(screen.getByText("CodexAccountsSourceAmbient")).toBeDefined();
  });

  it("shows the usage pill and blocked state from a snapshot", async () => {
    tauriMocks.getCodexAccountsState.mockResolvedValue(
      {
        accounts: [account("1")],
        snapshots: {
          "1": snapshot(38),
        },
      } as CodexAccountsStateBridge,
    );
    render(<CodexAccountsSection t={t} />);
    await waitFor(() => {
      expect(screen.getByText("free · 38%")).toBeDefined();
    });
  });

  it("adds an account and reloads", async () => {
    tauriMocks.getCodexAccountsState.mockResolvedValueOnce(
      { accounts: [], snapshots: {} } as CodexAccountsStateBridge,
    );
    tauriMocks.getCodexAccountsState.mockResolvedValueOnce(
      { accounts: [account("1")], snapshots: {} } as CodexAccountsStateBridge,
    );
    render(<CodexAccountsSection t={t} />);
    await waitFor(() => {
      expect(screen.getByText("CodexAccountsAddButton")).toBeDefined();
    });

    tauriMocks.codexAccountAdd.mockResolvedValue(account("1"));
    await act(async () => {
      screen.getByText("CodexAccountsAddButton").click();
    });
    await waitFor(() => {
      expect(screen.getByText("user-1@example.com")).toBeDefined();
    });
    expect(tauriMocks.codexAccountAdd).toHaveBeenCalledTimes(1);
  });

  it("switches an account and offers a desktop restart when a session can be restored", async () => {
    tauriMocks.getCodexAccountsState.mockResolvedValue(
      { accounts: [account("1")], snapshots: {} } as CodexAccountsStateBridge,
    );
    tauriMocks.codexAccountSwitch.mockResolvedValue(
      { desktopSessionRestoreExists: true, desktopSessionRestorePath: "C:/s", desktopSessionBackupPath: null } as CodexSwitchResult,
    );
    render(<CodexAccountsSection t={t} />);
    await waitFor(() => {
      expect(screen.getByText("CodexAccountsSwitchButton")).toBeDefined();
    });

    await act(async () => {
      screen.getByText("CodexAccountsSwitchButton").click();
    });
    await waitFor(() => {
      expect(screen.getByText(/CodexSwitchSuccess/)).toBeDefined();
    });
    expect(screen.getByText(/CodexSwitchRestartPrompt/)).toBeDefined();

    await act(async () => {
      screen.getByText("CodexAccountsRestartDesktop").click();
    });
    expect(tauriMocks.codexAccountRestartDesktop).toHaveBeenCalledTimes(1);
  });
});