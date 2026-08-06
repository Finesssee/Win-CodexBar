import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type {
  CodexAccount,
  CodexAccountsStateBridge,
  CodexAccountUsageSnapshot,
} from "../types/bridge";
import { useLocale } from "../hooks/useLocale";
import { maskEmail } from "./MenuCard";
import {
  codexAccountSwitch,
  getCodexAccountsState,
  refreshProviders,
} from "../lib/tauri";

/**
 * Multi-account lane surface for the Codex tray menu card (ADR 0003,
 * option A). Renders only when more than one Codex account exists, so the
 * common single-account menu stays unchanged (single-account fallback).
 *
 * Shows every account (ambient + managed) with a compact usage bar and a
 * Switch action. Switching updates the ambient identity and triggers a
 * provider refresh so the tray icon/menu reflect the now-active account.
 */
export default function CodexAccountsMenu({ hideEmail }: { hideEmail: boolean }) {
  const { t } = useLocale();
  const [accounts, setAccounts] = useState<CodexAccount[]>([]);
  const [snapshots, setSnapshots] = useState<
    Record<string, CodexAccountUsageSnapshot>
  >({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const next: CodexAccountsStateBridge = await getCodexAccountsState();
      setAccounts(next.accounts);
      setSnapshots(next.snapshots);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

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

  const handleSwitch = async (id: string) => {
    setBusy(true);
    setError(null);
    try {
      await codexAccountSwitch(id);
      await load();
      // Make the tray icon/menu reflect the newly active ambient identity.
      void refreshProviders().catch(() => {});
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  if (accounts.length <= 1) {
    return null;
  }

  return (
    <details className="codex-menu-accounts">
      <summary className="codex-menu-accounts__summary">
        <span className="codex-menu-accounts__title">{t("CodexAccountsTitle")}</span>
        <span className="codex-menu-accounts__count">{accounts.length}</span>
      </summary>
      {error && (
        <div className="codex-menu-accounts__error" role="alert">
          {error}
        </div>
      )}
      <ul className="codex-menu-accounts__list">
        {accounts.map((account) => {
          const snapshot = snapshots[account.id];
          // Prefer the primary (session) window, but accounts whose backend
          // only returns a weekly window have primaryWindow: null — fall back
          // to the next filled window in canonical order (primary →
          // secondary; the account-snapshot bridge carries no tertiary or
          // extra rate windows) so the usage bar still renders.
          const usageWindow =
            snapshot?.primaryWindow ?? snapshot?.secondaryWindow ?? null;
          const pct = usageWindow
            ? Math.round(usageWindow.usedPercent)
            : null;
          const label =
            account.nickname ??
            account.emailHint ??
            account.authSubject ??
            shrink(account.id);
          const shown = hideEmail ? maskEmail(label) : label;
          const isAmbient = account.source === "ambient";
          return (
            <li key={account.id}>
              <div
                className={`codex-menu-accounts__row${isAmbient ? " codex-menu-accounts__row--active" : ""}`}
              >
                <div className="codex-menu-accounts__meta">
                  <span className="codex-menu-accounts__email" title={label}>
                    {shown}
                    {isAmbient && (
                      <span className="codex-menu-accounts__badge">
                        {t("CodexAccountsSourceAmbient")}
                      </span>
                    )}
                  </span>
                  {pct !== null && (
                    <span className="codex-menu-accounts__bar" aria-hidden>
                      <span
                        className="codex-menu-accounts__bar-fill"
                        style={{ width: `${Math.max(2, Math.min(100, pct))}%` }}
                      />
                    </span>
                  )}
                </div>
                <button
                  type="button"
                  className="codex-menu-accounts__switch"
                  disabled={busy || isAmbient}
                  onClick={() => void handleSwitch(account.id)}
                >
                  {t("CodexAccountsSwitchButton")}
                </button>
              </div>
            </li>
          );
        })}
      </ul>
    </details>
  );
}

function shrink(id: string): string {
  return id.length <= 12 ? id : `${id.slice(0, 8)}…`;
}