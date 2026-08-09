import { useCallback, useEffect, useRef, useState } from "react";
import { useLocale } from "../../../hooks/useLocale";
import { getUsageSpendSummary } from "../../../lib/tauri";
import type { UsageSpendSummary } from "../../../types/bridge";
import type { TabProps } from "../settingsTabs";

const currencyFormatters = new Map<string, Intl.NumberFormat>();

function formatUsd(value: number | null | undefined, currency: string): string {
  if (value == null || !Number.isFinite(value)) return "—";
  const code = currency || "USD";
  try {
    let formatter = currencyFormatters.get(code);
    if (!formatter) {
      formatter = new Intl.NumberFormat(undefined, {
        style: "currency",
        currency: code,
        maximumFractionDigits: 2,
      });
      currencyFormatters.set(code, formatter);
    }
    return formatter.format(value);
  } catch {
    return `$${value.toFixed(2)}`;
  }
}

/** Sanitized share-card PNG (no account emails) — upstream #2112. */
function renderSharePng(summary: UsageSpendSummary, title: string): string {
  const rows = summary.rows ?? [];
  const pad = 24;
  const rowH = 28;
  const headerH = 48;
  const colW = [160, 100, 100, 80, 160];
  const width = pad * 2 + colW.reduce((a, b) => a + b, 0);
  const height = pad * 2 + headerH + Math.max(1, rows.length) * rowH + 36;
  const canvas = document.createElement("canvas");
  canvas.width = width * 2;
  canvas.height = height * 2;
  const ctx = canvas.getContext("2d");
  if (!ctx) return "";
  ctx.scale(2, 2);

  // Background
  ctx.fillStyle = "#0f1419";
  ctx.fillRect(0, 0, width, height);
  ctx.strokeStyle = "#243044";
  ctx.lineWidth = 1;
  ctx.strokeRect(0.5, 0.5, width - 1, height - 1);

  ctx.fillStyle = "#e7ecf3";
  ctx.font = "600 16px system-ui,Segoe UI,sans-serif";
  ctx.fillText(title, pad, pad + 18);

  ctx.fillStyle = "#8b9bb4";
  ctx.font = "12px system-ui,Segoe UI,sans-serif";
  ctx.fillText("Win-CodexBar · local estimates · no account emails", pad, pad + 36);

  const headers = ["Provider", "7 days", "30 days", "Currency", "Source"];
  let x = pad;
  const y0 = pad + headerH;
  ctx.fillStyle = "#9fb0c8";
  ctx.font = "600 12px system-ui,Segoe UI,sans-serif";
  headers.forEach((h, i) => {
    ctx.fillText(h, x, y0);
    x += colW[i];
  });

  ctx.strokeStyle = "#243044";
  ctx.beginPath();
  ctx.moveTo(pad, y0 + 8);
  ctx.lineTo(width - pad, y0 + 8);
  ctx.stroke();

  ctx.font = "13px system-ui,Segoe UI,sans-serif";
  if (rows.length === 0) {
    ctx.fillStyle = "#8b9bb4";
    ctx.fillText("No spend data yet.", pad, y0 + rowH);
  } else {
    rows.forEach((row, idx) => {
      const y = y0 + (idx + 1) * rowH;
      const cells = [
        row.displayName,
        formatUsd(row.sevenDay, row.currency),
        formatUsd(row.thirtyDay, row.currency),
        row.currency || "USD",
        row.source,
      ];
      let cx = pad;
      cells.forEach((cell, i) => {
        ctx.fillStyle = i === 0 ? "#e7ecf3" : "#c5d0e0";
        const text = String(cell);
        const max = colW[i] - 8;
        let draw = text;
        if (ctx.measureText(draw).width > max) {
          while (draw.length > 1 && ctx.measureText(`${draw}…`).width > max) {
            draw = draw.slice(0, -1);
          }
          draw = `${draw}…`;
        }
        ctx.fillText(draw, cx, y);
        cx += colW[i];
      });
    });
  }

  return canvas.toDataURL("image/png");
}

function downloadDataUrl(dataUrl: string, filename: string) {
  const a = document.createElement("a");
  a.href = dataUrl;
  a.download = filename;
  a.rel = "noopener";
  document.body.appendChild(a);
  a.click();
  a.remove();
}

export default function UsageSpendTab(_props: TabProps) {
  const { t } = useLocale();
  const [summary, setSummary] = useState<UsageSpendSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [shareError, setShareError] = useState<string | null>(null);
  const tableRef = useRef<HTMLTableElement | null>(null);

  const load = useCallback(() => {
    setLoading(true);
    setError(null);
    void getUsageSpendSummary()
      .then((data) => {
        setSummary(data);
        setLoading(false);
      })
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : String(err));
        setLoading(false);
      });
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const onShare = useCallback(() => {
    setShareError(null);
    if (!summary) {
      setShareError(t("UsageSpendShareEmpty"));
      return;
    }
    try {
      const dataUrl = renderSharePng(summary, t("UsageSpendTitle"));
      if (!dataUrl) {
        setShareError(t("UsageSpendShareFailed"));
        return;
      }
      const stamp = new Date().toISOString().slice(0, 10);
      downloadDataUrl(dataUrl, `codexbar-usage-spend-${stamp}.png`);
    } catch {
      setShareError(t("UsageSpendShareFailed"));
    }
  }, [summary, t]);

  return (
    <section className="settings-section">
      <h3 className="settings-section__title settings-section__title--bold">
        {t("UsageSpendTitle")}
      </h3>
      <p className="settings-section__caption">{t("UsageSpendCaption")}</p>

      <div className="settings-section__group" style={{ marginBottom: 12, display: "flex", gap: 8 }}>
        <button
          type="button"
          className="credential-btn credential-btn--secondary"
          disabled={loading}
          onClick={load}
        >
          {loading ? t("UsageSpendLoading") : t("UsageSpendRefresh")}
        </button>
        <button
          type="button"
          className="credential-btn credential-btn--secondary"
          disabled={loading || !summary}
          onClick={onShare}
        >
          {t("UsageSpendShare")}
        </button>
      </div>

      {error && <p className="settings-section__error">{error}</p>}
      {shareError && <p className="settings-section__error">{shareError}</p>}

      {!error && (
        <table className="usage-spend-table" ref={tableRef}>
          <thead>
            <tr>
              <th>{t("UsageSpendColProvider")}</th>
              <th>{t("UsageSpendCol7d")}</th>
              <th>{t("UsageSpendCol30d")}</th>
              <th>{t("UsageSpendColCurrency")}</th>
              <th>{t("UsageSpendColSource")}</th>
            </tr>
          </thead>
          <tbody>
            {(summary?.rows ?? []).map((row) => (
              <tr key={row.providerId}>
                <td>{row.displayName}</td>
                <td>{formatUsd(row.sevenDay, row.currency)}</td>
                <td>{formatUsd(row.thirtyDay, row.currency)}</td>
                <td>{row.currency || "USD"}</td>
                <td className="usage-spend-table__source">
                  {row.source}
                  {row.refreshing && (
                    <span className="usage-spend-table__refreshing" title={row.staleUpdatedAt ? `Stale as of ${row.staleUpdatedAt}` : undefined}>
                      {" · "}{t("UsageSpendRefreshing")}
                    </span>
                  )}
                </td>
              </tr>
            ))}
            {!loading && (summary?.rows?.length ?? 0) === 0 && (
              <tr>
                <td colSpan={5}>{t("UsageSpendEmpty")}</td>
              </tr>
            )}
          </tbody>
        </table>
      )}
    </section>
  );
}
