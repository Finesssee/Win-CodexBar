import { useCallback, useEffect, useState } from "react";
import { focusAgentSession, listAgentSessions } from "../lib/tauri";
import type {
  AgentSession,
  AgentSessionDiscoveryResult,
} from "../types/bridge";
import { useLocale } from "../hooks/useLocale";

export default function AgentSessions() {
  const { t } = useLocale();
  const [result, setResult] = useState<AgentSessionDiscoveryResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    setError(null);
    void listAgentSessions()
      .then(setResult)
      .catch((reason: unknown) =>
        setError(reason instanceof Error ? reason.message : String(reason)),
      );
  }, []);

  useEffect(refresh, [refresh]);

  const sessionLabel = (session: AgentSession): string => {
    if (session.provider === "codex") return t("ProviderNameCodex");
    if (session.provider === "claude") return t("ProviderNameClaude");
    return session.dialect === "omp"
      ? t("AgentSessionsProviderOmp")
      : t("AgentSessionsProviderPi");
  };

  if (result?.status === "disabled") return null;
  const hosts = result?.status === "hosts" ? result.hosts : [];
  const sessions = hosts.flatMap((host) => host.sessions);
  const hostErrors = hosts.flatMap((host) =>
    host.error ? [`${host.host}: ${host.error}`] : [],
  );

  const focus = (session: AgentSession) => {
    void focusAgentSession(session).then((focusResult) => {
      if (focusResult.status !== "focused") setError(focusResult.message);
    });
  };

  return (
    <section className="agent-sessions" aria-label={t("AgentSessionsTitle")}>
      <div className="agent-sessions__header">
        <strong>{t("AgentSessionsTitle")}</strong>
        <button type="button" onClick={refresh}>{t("ActionRefresh")}</button>
      </div>
      {!result && <p>{t("AgentSessionsLoading")}</p>}
      {result && sessions.length === 0 && (
        <p>{t("AgentSessionsEmpty")}</p>
      )}
      {sessions.map((session) => (
        <button
          type="button"
          className="agent-sessions__row"
          key={`${session.host}:${session.provider}:${session.id}`}
          onClick={() => focus(session)}
        >
          <span>{sessionLabel(session)}</span>
          <span>
            {session.sessionName ?? session.workspace.projectName ?? session.host}
          </span>
          <span>{session.state}</span>
        </button>
      ))}
      {[...hostErrors, ...(error ? [error] : [])].map((message) => (
        <p className="agent-sessions__error" key={message}>{message}</p>
      ))}
    </section>
  );
}
