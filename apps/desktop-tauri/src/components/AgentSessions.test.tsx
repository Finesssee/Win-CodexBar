import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  listAgentSessions: vi.fn(),
  focusAgentSession: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({
  listAgentSessions: api.listAgentSessions,
  focusAgentSession: api.focusAgentSession,
}));
// t(key) returns the key, so provider labels assert against locale-key names.
vi.mock("../hooks/useLocale", () => ({
  useLocale: () => ({ t: (key: string) => key, language: "english" }),
}));

import AgentSessions from "./AgentSessions";
import type { AgentSession } from "../types/bridge";

function session(overrides: Partial<AgentSession>): AgentSession {
  return {
    id: "s",
    provider: "codex",
    source: "cli",
    state: "active",
    pid: 1,
    transcriptPath: null,
    host: "DESKTOP",
    workspace: { cwd: null, projectName: "proj" },
    activity: { startedAt: null, lastActivityAt: null },
    focusTarget: { kind: "process", pid: 1 },
    ...overrides,
  };
}

describe("AgentSessions", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders codex and claude provider labels from locale keys", async () => {
    api.listAgentSessions.mockResolvedValue({
      status: "hosts",
      hosts: [
        {
          host: "DESKTOP",
          sessions: [
            session({ id: "1", provider: "codex" }),
            session({ id: "2", provider: "claude" }),
          ],
          error: null,
        },
      ],
    });
    render(<AgentSessions />);

    expect(await screen.findByText("ProviderNameCodex")).toBeTruthy();
    expect(await screen.findByText("ProviderNameClaude")).toBeTruthy();
  });

  it("labels pi-family sessions by dialect and prefers sessionName", async () => {
    api.listAgentSessions.mockResolvedValue({
      status: "hosts",
      hosts: [
        {
          host: "DESKTOP",
          sessions: [
            session({ id: "pid:7", provider: "pi", dialect: "pi" }),
            session({
              id: "omp-fx",
              provider: "pi",
              dialect: "omp",
              sessionName: "OMP fixture",
            }),
          ],
          error: null,
        },
      ],
    });
    render(<AgentSessions />);

    expect(await screen.findByText("AgentSessionsProviderPi")).toBeTruthy();
    expect(await screen.findByText("AgentSessionsProviderOmp")).toBeTruthy();
    // sessionName shown when present; project fallback otherwise.
    expect(await screen.findByText("OMP fixture")).toBeTruthy();
    expect(
      (await screen.findAllByText("proj")).length > 0,
    ).toBeTruthy();
  });

  it("defaults bare pi sessions (no dialect field emitted) to the family label", async () => {
    api.listAgentSessions.mockResolvedValue({
      status: "hosts",
      hosts: [
        {
          host: "DESKTOP",
          sessions: [session({ id: "pid:9", provider: "pi" })],
          error: null,
        },
      ],
    });
    render(<AgentSessions />);

    expect(await screen.findByText("AgentSessionsProviderPi")).toBeTruthy();
  });
});
