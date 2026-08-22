import { useEffect, useState } from "react";
import type { LocaleKey } from "../../../../../i18n/keys";
import {
  hasOpenRouterManagementApiKey,
  removeOpenRouterManagementApiKey,
  setOpenRouterManagementApiKey,
} from "../../../../../lib/tauri";

interface Props {
  t: (key: LocaleKey) => string;
}

export function OpenRouterManagementCreds({ t }: Props) {
  const [configured, setConfigured] = useState(false);
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void hasOpenRouterManagementApiKey()
      .then((present) => {
        if (!cancelled) setConfigured(present);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const save = async () => {
    const next = value.trim();
    if (!next) return;
    setBusy(true);
    setError(null);
    try {
      await setOpenRouterManagementApiKey(next);
      setConfigured(true);
      setValue("");
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    setError(null);
    try {
      await removeOpenRouterManagementApiKey();
      setConfigured(false);
      setValue("");
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="provider-detail-section">
      <h4>{t("OpenRouterManagementKeyTitle")}</h4>
      <label className="provider-detail-field">
        <span className="provider-detail-field__label">
          {t("OpenRouterManagementKeyLabel")}
        </span>
        <input
          className="provider-detail-field__input"
          type="password"
          value={value}
          autoComplete="off"
          spellCheck={false}
          placeholder={configured ? t("OpenRouterManagementKeyConfigured") : "or-..."}
          aria-label={t("OpenRouterManagementKeyLabel")}
          onChange={(event) => setValue(event.target.value)}
        />
      </label>
      <div className="provider-detail-helper">{t("OpenRouterManagementKeyHelp")}</div>
      <div className="provider-detail-actions">
        <button
          type="button"
          className="credential-btn credential-btn--primary"
          disabled={busy || value.trim().length === 0}
          onClick={() => void save()}
        >
          {t("Save")}
        </button>
        {configured && (
          <button
            type="button"
            className="credential-btn credential-btn--secondary"
            disabled={busy}
            onClick={() => void remove()}
          >
            {t("Remove")}
          </button>
        )}
      </div>
      {error && <div className="provider-detail-error">{error}</div>}
    </section>
  );
}
