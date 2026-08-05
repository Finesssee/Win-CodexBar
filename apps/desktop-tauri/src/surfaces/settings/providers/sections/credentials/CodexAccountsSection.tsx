import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type {
  CodexAccount,
  CodexAccountsStateBridge,
  CodexAccountUsageSnapshot,
  CodexSwitchResult,
} from "../../../../../types/bridge";
import type { LocaleKey } from "../../../../../i18n/keys";
import {
  codexAccountAdd,
  codexAccountFetch,
  codexAccountRemove,
  codexAccountRestartDesktop,
  codexAccountSwitch,
  getCodexAccountsState,
} from "../../../../../lib/tauri";

interface Props {
  t: (key: LocaleKey) => string;
}

/**
 * Inline "Codex Accounts" surface shown inside the Settings → Providers →
 * Codex detail pane.
 *
 * Multi-account Codex support (ADR 0003). Reads the shared account +
 * snapshot store via `get_codex_accounts_state` and drives the
 * `codex_account_*` IPC surface: add (login into a managed home), switch the
 * active ambient identity, refresh per-account usage, and remove managed
 * homes. For MSIX Codex Desktop installs a restart action is offered when a
 * session snapshot is available to restore.
 */
export function CodexAccountsSection({ t }: Props) {
  const [accounts, setAccounts] = useState<CodexAccount[]>([]);
  const [snapshots, setSnapshots] = useState<
    Record<string, CodexAccountUsageSnapshot>
  >({});
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [switchResult, setSwitchResult] = useState<CodexSwitchResult | null>(
    null,
  );

  const load = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const next: CodexAccountsStateBridge = await getCodexAccountsState();
      setAccounts(next.accounts);
      setSnapshots(next.snapshots);
      setLoaded(true);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // Live-refresh after the provider engine runs the per-account lanes
  // (ADR 0003 multi-account refresh) so the panel stays current.
  useEffect(() => {
    let cancelled = false;
    const unlistenPromise = listen("codex-accounts-updated", () => {
      if (!cancelled) void load();
    });
    return () => {
      cancelled = true;
      void unlistenPromise.then((fn) => fn());
    };
  }, [load]);

  const handleAdd = async () => {
    setBusy(true);
    setError(null);
    setSwitchResult(null);
    try {
      await codexAccountAdd();
      await load();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleSwitch = async (id: string) => {
    setBusy(true);
    setError(null);
    setSwitchResult(null);
    try {
      const result = await codexAccountSwitch(id);
      setSwitchResult(result);
      await load();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleFetch = async (id: string) => {
    setBusy(true);
    setError(null);
    try {
      const snapshot = await codexAccountFetch(id);
      setSnapshots((prev) => ({ ...prev, [id]: snapshot }));
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleRemove = async (id: string) => {
    setBusy(true);
    setError(null);
    setSwitchResult(null);
    try {
      await codexAccountRemove(id);
      await load();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleRestartDesktop = async () => {
    if (!switchResult) return;
    setBusy(true);
    setError(null);
    try {
      await codexAccountRestartDesktop(
        null,
        switchResult.desktopSessionBackupPath ?? null,
        switchResult.desktopSessionRestorePath ?? null,
      );
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  if (!loaded) {
    return null;
  }

  return (
    <section className="provider-detail-section codex-accounts">
      <div className="provider-detail-section__header">
        <h4>{t("CodexAccountsTitle")}</h4>
        {accounts.length === 0 && (
          <button
            type="button"
            className="credential-btn credential-btn--primary"
            disabled={busy}
            onClick={() => void handleAdd()}
          >
            {t("CodexAccountsAddButton")}
          </button>
        )}
      </div>
      <p className="settings-section__hint">{t("CodexAccountsHint")}</p>

      {error && (
        <div className="provider-detail-error" role="alert">
          {error}
        </div>
      )}

      {switchResult && (
        <div className="provider-detail-note" role="status">
          {t("CodexSwitchSuccess")}
          {switchResult.desktopSessionRestoreExists && (
            <>
              {" "}
              {t("CodexSwitchRestartPrompt")}{" "}
              <button
                type="button"
                className="credential-btn credential-btn--secondary"
                disabled={busy}
                onClick={() => void handleRestartDesktop()}
              >
                {t("CodexAccountsRestartDesktop")}
              </button>
            </>
          )}
        </div>
      )}

      {accounts.length === 0 ? (
        <p className="credential-empty">{t("CodexAccountsEmpty")}</p>
      ) : (
        <>
          <ul className="credential-list codex-accounts-list">
            {accounts.map((account) => {
              const snapshot = snapshots[account.id];
              return (
                <li
                  key={account.id}
                  className="credential-card codex-accounts-card"
                >
                  <div className="credential-card__header">
                    <div className="credential-card__info">
                      <strong>
                        {account.nickname ??
                          account.emailHint ??
                          account.authSubject ??
                          shrink(account.id)}
                      </strong>
                      <span className="credential-card__meta">
                        <span className="credential-card__badge credential-card__badge--set">
                          {account.source === "ambient"
                            ? t("CodexAccountsSourceAmbient")
                            : t("CodexAccountsSourceManaged")}
                        </span>
                        {snapshot ? (
                          <CodexUsagePill snapshot={snapshot} t={t} />
                        ) : (
                          <span className="credential-card__date">
                            {t("CodexAccountsUsageUnavailable")}
                          </span>
                        )}
                      </span>
                    </div>
                    <div className="credential-card__actions">
                      <button
                        type="button"
                        className="credential-btn credential-btn--secondary"
                        disabled={busy}
                        onClick={() => void handleFetch(account.id)}
                      >
                        {t("CodexAccountsFetchButton")}
                      </button>
                      <button
                        type="button"
                        className="credential-btn credential-btn--primary"
                        disabled={busy}
                        onClick={() => void handleSwitch(account.id)}
                      >
                        {t("CodexAccountsSwitchButton")}
                      </button>
                      <button
                        type="button"
                        className="credential-btn credential-btn--danger"
                        disabled={busy}
                        onClick={() => void handleRemove(account.id)}
                      >
                        {t("CodexAccountsRemoveButton")}
                      </button>
                    </div>
                  </div>
                </li>
              );
            })}
          </ul>
          <div className="codex-accounts-add">
            <button
              type="button"
              className="credential-btn credential-btn--primary"
              disabled={busy}
              onClick={() => void handleAdd()}
            >
              {t("CodexAccountsAddButton")}
            </button>
          </div>
        </>
      )}
    </section>
  );
}

function shrink(id: string): string {
  return id.length <= 12 ? id : `${id.slice(0, 8)}…`;
}

function CodexUsagePill({
  snapshot,
  t,
}: {
  snapshot: CodexAccountUsageSnapshot;
  t: (key: LocaleKey) => string;
}) {
  const window = snapshot.primaryWindow;
  const percent = window ? Math.round(window.usedPercent) : null;
  const plan = snapshot.plan ?? "";
  const blocked = snapshot.allowed === false || snapshot.limitReached === true;
  const label = [plan, percent !== null ? `${percent}%` : null]
    .filter(Boolean)
    .join(" · ");
  return (
    <span className={blocked ? "codex-usage codex-usage--blocked" : "codex-usage"}>
      {label || t("CodexAccountsUsageUnavailable")}
    </span>
  );
}