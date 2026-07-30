import { Fragment, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { BootstrapState, ProviderUsageSnapshot } from "../types/bridge";
import {
  beginFlyoutGesture,
  dismissTrayPanel,
  endFlyoutGesture,
  flyoutStoredSize,
  openSettingsWindow,
  quitApp as quitApplication,
  reorderProviders,
  setFlyoutSize,
  setSurfaceMode,
  updateSettings,
} from "../lib/tauri";
import { useProviders } from "../hooks/useProviders";
import { useSettings } from "../hooks/useSettings";
import { useUpdateState } from "../hooks/useUpdateState";
import { useLocale } from "../hooks/useLocale";
import { useSurfaceTarget } from "../hooks/useSurfaceMode";
import { useTrayPanelLayout } from "../hooks/useTrayPanelLayout";
import MenuCard from "../components/MenuCard";
import MenuSurface, {
  MenuEmpty,
  type MenuFooterRow,
} from "../components/MenuSurface";
import UpdateBanner from "../components/UpdateBanner";
import ProviderGrid, { prioritizeProviders } from "../components/ProviderGrid";
import { openProviderDashboard, openProviderStatusPage } from "../lib/tauri";
import { orderProviderSnapshots } from "../lib/providerOrder";
import {
  hydrateProviderSlots,
  orderedEnabledProviderSlots,
} from "../lib/trayProviders";
import AgentSessions from "../components/AgentSessions";

/** Provider IDs that have a dashboard URL in the backend */
const HAS_DASHBOARD = new Set([
  "abacus", "alibaba", "alibabatokenplan", "amp", "augment",
  "azureopenai", "bedrock", "claude", "codex", "codebuff",
  "aiand", "commandcode", "copilot", "crof", "crossmodel", "cursor", "deepgram", "deepinfra", "deepseek", "zenmux", "clinepass", "longcat", "neuralwatt", "zoommate",
  "doubao", "elevenlabs", "factory", "gemini", "grok", "groq",
  "infini", "jetbrains", "kilo", "kimi", "kimik2", "kiro", "manus",
  "mimo", "minimax", "mistral", "nanogpt", "ollama", "openaiapi",
  "opencode", "opencodego", "openrouter", "perplexity", "qoder", "sakana", "stepfun",
  "t3chat", "venice", "vertexai", "warp", "windsurf",
  "zai",
]);
/** Provider IDs that have a status page URL in the backend */
const HAS_STATUS_PAGE = new Set([
  "alibabatokenplan", "amp", "augment", "azureopenai", "bedrock",
  "claude", "codex", "copilot", "deepgram", "deepinfra", "deepseek", "zenmux", "clinepass", "longcat", "neuralwatt", "zoommate", "elevenlabs",
  "gemini", "grok", "groq", "kiro", "mistral", "openaiapi",
  "openrouter", "vertexai", "windsurf",
]);

const TRAY_INITIAL_REFRESH_DELAY_MS = 250;
const DENSE_OVERVIEW_THRESHOLD = 32;

// ── Tray flyout zoom (footer slider, above Refresh) ───────────────────
// PopOut window mode has its own independent windowScalePercent (webview
// setZoom) — this is a separate setting/control for the tray flyout only,
// applied via CSS `zoom` on the MenuSurface root (see render below).
const TRAY_SCALE_MIN = 100;
const TRAY_SCALE_MAX = 200;
const TRAY_SCALE_STEP = 5;
const TRAY_SCALE_COMMIT_DEBOUNCE_MS = 250;

function clampTrayScalePercent(value: number): number {
  return Math.min(
    TRAY_SCALE_MAX,
    Math.max(TRAY_SCALE_MIN, Number.isFinite(value) ? value : 100),
  );
}

function getProviderStatus(
  p: ProviderUsageSnapshot,
): "ok" | "warning" | "exhausted" | "error" {
  if (p.error) return "error";
  if (p.primary.isExhausted) return "exhausted";
  if (p.primary.usedPercent > 80) return "warning";
  return "ok";
}
void getProviderStatus;

/**
 * Tray popover surface — two modes like macOS CodexBar:
 * 1. Overview (default): provider grid + all cards stacked
 * 2. Detail: click a provider in grid → show only that provider's card
 */
export default function TrayPanel({ state }: { state: BootstrapState }) {
  const { settings } = useSettings(state.settings);
  const {
    providers,
    isRefreshing,
    refreshingProviderIds,
    refresh,
    hasCachedData,
    hasLoadedCache,
  } = useProviders({
    initialRefreshDelayMs: TRAY_INITIAL_REFRESH_DELAY_MS,
    forceRefreshOnMount: settings.refreshAllProvidersOnMenuOpen,
  });
  const { updateState, checkNow, download, apply, dismiss, openRelease } =
    useUpdateState();

  const { t } = useLocale();
  const surfaceTarget = useSurfaceTarget("trayPanel");

  // Zoom slider: LOCAL draft state drives both the thumb and the live CSS
  // zoom preview while dragging; persistence trails behind a ~250ms debounce
  // (fire-and-forget updateSettings). The settings_changed echo — from our
  // own commit round-trip or another window — only re-syncs the draft when
  // no debounce is pending, so it can't fight the thumb mid-drag.
  const settingsTrayScalePercent = clampTrayScalePercent(
    settings.trayScalePercent,
  );
  const [trayScaleDraft, setTrayScaleDraft] = useState(
    settingsTrayScalePercent,
  );
  const trayScaleCommitTimerRef = useRef<number | undefined>(undefined);
  useEffect(() => {
    if (trayScaleCommitTimerRef.current === undefined) {
      setTrayScaleDraft(settingsTrayScalePercent);
    }
  }, [settingsTrayScalePercent]);
  useEffect(
    () => () => {
      if (trayScaleCommitTimerRef.current !== undefined) {
        window.clearTimeout(trayScaleCommitTimerRef.current);
      }
    },
    [],
  );
  const handleTrayScaleChange = useCallback((value: number) => {
    const next = clampTrayScalePercent(value);
    setTrayScaleDraft(next);
    if (trayScaleCommitTimerRef.current !== undefined) {
      window.clearTimeout(trayScaleCommitTimerRef.current);
    }
    trayScaleCommitTimerRef.current = window.setTimeout(() => {
      trayScaleCommitTimerRef.current = undefined;
      void updateSettings({ trayScalePercent: next }).catch(() => {});
    }, TRAY_SCALE_COMMIT_DEBOUNCE_MS);
  }, []);
  const trayScale = trayScaleDraft / 100;
  const trayScaleFillPercent =
    ((trayScaleDraft - TRAY_SCALE_MIN) / (TRAY_SCALE_MAX - TRAY_SCALE_MIN)) *
    100;
  const zoomRow = (
    <div className="menu-surface__footer-row menu-surface__footer-zoom">
      <span>{t("PanelZoom")}</span>
      <input
        type="range"
        className="menu-surface__footer-zoom-slider"
        min={TRAY_SCALE_MIN}
        max={TRAY_SCALE_MAX}
        step={TRAY_SCALE_STEP}
        value={trayScaleDraft}
        aria-label={t("PanelZoom")}
        onChange={(e) => handleTrayScaleChange(Number(e.target.value))}
        style={{ "--zoom-fill": `${trayScaleFillPercent}%` } as CSSProperties}
      />
      <span className="menu-surface__footer-zoom-value">
        {trayScaleDraft}%
      </span>
    </div>
  );

  const sorted = useMemo(
    () =>
      orderProviderSnapshots(
        providers,
        state.providers,
        settings.enabledProviders,
        settings.providerOrder,
      ),
    [providers, settings.enabledProviders, settings.providerOrder, state.providers],
  );
  const denseProviderSlots = useMemo(
    () =>
      orderedEnabledProviderSlots(
        state.providers,
        settings.enabledProviders,
        sorted,
        settings.providerOrder,
      ),
    [settings.enabledProviders, settings.providerOrder, sorted, state.providers],
  );
  const providersById = useMemo(
    () => new Map(sorted.map((provider) => [provider.providerId, provider])),
    [sorted],
  );
  const initialProviderId =
    surfaceTarget?.kind === "provider" ? surfaceTarget.providerId : null;

  // null = overview (all providers), string = single provider detail
  const [selectedProviderId, setSelectedProviderId] = useState<string | null>(
    initialProviderId,
  );
  const [gridExpanded, setGridExpanded] = useState(false);
  const expectsDenseOverview =
    selectedProviderId === null &&
    !gridExpanded &&
    settings.enabledProviders.length + 1 > DENSE_OVERVIEW_THRESHOLD;
  const denseTrayProviders = useMemo(() => {
    if (!expectsDenseOverview) return sorted;
    return hydrateProviderSlots(denseProviderSlots, providersById);
  }, [denseProviderSlots, expectsDenseOverview, providersById, sorted]);

  useEffect(() => {
    setSelectedProviderId(initialProviderId);
  }, [initialProviderId]);

  // Cards to display based on mode
  // Overview: all providers in the grid — non-error first, then errors
  // Detail: only the selected provider's card (macOS shows single provider)
  const visibleProviders = useMemo(() => {
    if (selectedProviderId === null) {
      // Overview: show providers in the same Settings/catalog order as the grid.
      if (sorted.length + 1 > DENSE_OVERVIEW_THRESHOLD && !gridExpanded) {
        return prioritizeProviders(denseTrayProviders, null).slice(0, 4);
      }
      return sorted;
    }
    // Detail: show ONLY the selected provider (macOS behavior — no appended errors)
    const match = sorted.find((p) => p.providerId === selectedProviderId);
    if (!match) {
      return sorted;
    }
    return [match];
  }, [denseTrayProviders, sorted, selectedProviderId, gridExpanded]);

  const layoutKey = useMemo(
    () =>
      [
        selectedProviderId ?? "overview",
        gridExpanded ? "expanded" : "collapsed",
        isRefreshing ? "refreshing" : "idle",
        updateState.status,
        updateState.version ?? "",
        updateState.error ?? "",
        expectsDenseOverview ? "dense" : "normal",
        hasLoadedCache ? "cache-ready" : "cache-pending",
        visibleProviders.map((provider) => provider.providerId).join(","),
        trayScaleDraft,
      ].join("|"),
    [
      selectedProviderId,
      gridExpanded,
      isRefreshing,
      updateState.status,
      updateState.version,
      updateState.error,
      expectsDenseOverview,
      hasLoadedCache,
      visibleProviders,
      trayScaleDraft,
    ],
  );

  // Flyout sizing: auto-fit to content until the user manually drags the border,
  // then remember + honor their size (position always re-anchors above the tray).
  // `flyoutSize`: undefined = loading, null = auto-fit, [w,h] = user's fixed size.
  const [flyoutSize, setFlyoutSizeState] = useState<
    [number, number] | null | undefined
  >(undefined);
  const [autoFitKilled, setAutoFitKilled] = useState(false);
  useEffect(() => {
    let active = true;
    void flyoutStoredSize()
      .then((size) => {
        if (active) setFlyoutSizeState(size);
      })
      .catch(() => {
        if (active) setFlyoutSizeState(null);
      });
    return () => {
      active = false;
    };
  }, []);

  const saveSizeTimerRef = useRef<number | undefined>(undefined);
  const handleUserResize = useCallback((width: number, height: number) => {
    // Stop auto-fit immediately so it can't fight the drag; commit the size
    // (state + persistence) after the drag settles.
    setAutoFitKilled(true);
    if (saveSizeTimerRef.current !== undefined) {
      window.clearTimeout(saveSizeTimerRef.current);
    }
    saveSizeTimerRef.current = window.setTimeout(() => {
      setFlyoutSizeState([width, height]);
      void setFlyoutSize(width, height).catch(() => {});
    }, 300);
  }, []);
  useEffect(
    () => () => {
      if (saveSizeTimerRef.current !== undefined) {
        window.clearTimeout(saveSizeTimerRef.current);
      }
    },
    [],
  );

  // TrayPanel now renders exclusively inside its own dedicated "flyout" OS
  // window (see App.tsx's isFlyoutWindow() routing) — it is no longer a
  // state of the shared `main` window's surface-mode machine. The old
  // `useSurfaceMode() === "trayPanel"` check would be permanently false
  // here (that machine now only tracks Hidden/PopOut/Settings on `main`),
  // which would silently gate off the fixed-size restore + reveal below
  // (useTrayPanelLayout's `isOpen` gate) — a user-resized flyout would never
  // reveal itself. Hardcoded true: being mounted IS "the flyout is open".
  const isFlyoutOpen = true;
  const fixedFlyoutSize = Array.isArray(flyoutSize) ? flyoutSize : null;
  const useWideColumns =
    selectedProviderId === null &&
    fixedFlyoutSize !== null &&
    fixedFlyoutSize[0] >= 640;
  const wideColumns = useMemo(() => {
    const columns: ProviderUsageSnapshot[][] = [[], []];
    visibleProviders.forEach((provider, index) => {
      columns[index % 2].push(provider);
    });
    return columns;
  }, [visibleProviders]);
  const { layoutReady, requestLayout } = useTrayPanelLayout({
    canMeasure: hasLoadedCache || sorted.length > 0,
    denseOverview: expectsDenseOverview,
    detailMode: selectedProviderId !== null,
    layoutKey,
    autoFit: flyoutSize === null && !autoFitKilled,
    fixedSize: fixedFlyoutSize,
    isOpen: isFlyoutOpen,
    onUserResize: handleUserResize,
  });

  const openSettings = useCallback(() => {
    void openSettingsWindow("general").finally(() => {
      void getCurrentWindow().close();
    });
  }, []);
  const openPopOut = useCallback(() => {
    setSurfaceMode("popOut", { kind: "dashboard" });
  }, []);
  const openAbout = useCallback(() => {
    void openSettingsWindow("about").finally(() => {
      void getCurrentWindow().close();
    });
  }, []);
  const quitApp = useCallback(() => {
    void quitApplication();
  }, []);

  const headerActions = [
    { icon: "⧉", title: t("TooltipPopOut"), onClick: openPopOut },
  ];

  const footerRows: MenuFooterRow[] = [
    { icon: "↻", label: t("ActionRefresh"), shortcut: "Ctrl+R", onClick: refresh },
    { icon: "⚙", label: t("MenuSettings"), shortcut: "Ctrl+,", onClick: openSettings },
    { icon: "ⓘ", label: t("MenuAbout"), onClick: openAbout },
    { icon: "⌧", label: t("MenuQuit"), shortcut: "Ctrl+Q", onClick: quitApp },
  ];

  // Keyboard shortcuts
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (
        e.key === "Escape" &&
        !e.ctrlKey &&
        !e.shiftKey &&
        !e.altKey &&
        !e.metaKey
      ) {
        e.preventDefault();
        void dismissTrayPanel().catch(() => {});
        return;
      }
      if (!e.ctrlKey || e.shiftKey || e.altKey || e.metaKey) return;
      switch (e.key.toLowerCase()) {
        case "r":
          e.preventDefault();
          refresh();
          break;
        case ",":
          e.preventDefault();
          openSettings();
          break;
        case "q":
          e.preventDefault();
          quitApp();
          break;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [refresh, openSettings, quitApp]);

  const handleGridClick = useCallback(
    (providerId: string | null) => {
      setSelectedProviderId(providerId);
    },
    [],
  );
  const handleReorder = useCallback((orderedIds: string[]) => {
    void reorderProviders(orderedIds).catch(() => {});
  }, []);
  const handleGestureStart = useCallback(() => {
    void beginFlyoutGesture().catch(() => {});
  }, []);
  const handleGestureEnd = useCallback(() => {
    void endFlyoutGesture().catch(() => {});
  }, []);
  const banner = (
    <UpdateBanner
      updateState={updateState}
      onCheck={checkNow}
      onDownload={download}
      onApply={apply}
      onDismiss={dismiss}
      onOpenRelease={openRelease}
    />
  );
  const revealClassName = `tray-panel-reveal${layoutReady ? " tray-panel-reveal--ready" : ""}${expectsDenseOverview ? " tray-panel-reveal--dense" : ""}${fixedFlyoutSize ? " tray-panel-reveal--usersized" : ""}`;
  const renderProviderCard = (p: ProviderUsageSnapshot) => {
    const isSelected =
      selectedProviderId !== null && p.providerId === selectedProviderId;
    return (
      <div
        className={`menu-stack__item${isSelected ? " menu-stack__item--selected" : ""}`}
        id={`card-${p.providerId}`}
        key={p.providerId}
      >
        <MenuCard
          provider={p}
          isRefreshing={refreshingProviderIds.has(p.providerId)}
          hideEmail={settings.hidePersonalInfo}
          resetTimeRelative={settings.resetTimeRelative}
          showResetWhenExhausted={settings.showResetWhenExhausted}
          showAsUsed={settings.showAsUsed}
          compactMetrics={selectedProviderId === null}
          onLayoutChange={requestLayout}
        />
      </div>
    );
  };

  if (sorted.length === 0) {
    return (
      <div className={revealClassName}>
        <MenuSurface
          variant="tray"
          onRefresh={refresh}
          isRefreshing={isRefreshing}
          actions={headerActions}
          banner={banner}
          footerLead={zoomRow}
          footerRows={footerRows}
          style={{ zoom: trayScale }}
        >
          {settings.agentSessionsEnabled && <AgentSessions />}
          <MenuEmpty
            isLoading={isRefreshing && !hasCachedData}
            onSettings={openSettings}
          />
        </MenuSurface>
        <TrayResizeHandles />
      </div>
    );
  }

  return (
    <div className={revealClassName}>
      <MenuSurface
        variant="tray"
        onRefresh={refresh}
        isRefreshing={isRefreshing}
        actions={headerActions}
        banner={banner}
        footerLead={zoomRow}
        footerRows={footerRows}
        style={{ zoom: trayScale }}
      >
        {settings.agentSessionsEnabled && <AgentSessions />}
        <ProviderGrid
          providers={expectsDenseOverview ? denseTrayProviders : sorted}
          selectedProviderId={selectedProviderId}
          showAsUsed={settings.showAsUsed}
          showProviderIcons={settings.switcherShowsIcons}
          expanded={gridExpanded}
          onExpandedChange={setGridExpanded}
          onSelect={handleGridClick}
          onReorder={handleReorder}
          onGestureStart={handleGestureStart}
          onGestureEnd={handleGestureEnd}
        />
        <div className="provider-grid__divider" />
        <div className="menu-stack">
          {useWideColumns
            ? wideColumns.map((column, index) => (
                <div className="menu-stack__column" key={index}>
                  {column.map(renderProviderCard)}
                </div>
              ))
            : visibleProviders.map((p, idx) => (
                <Fragment key={p.providerId}>
                  {idx > 0 && <div className="menu-stack__sep" />}
                  {renderProviderCard(p)}
                </Fragment>
              ))}
        </div>
        {/* Context actions — detail mode only, matches macOS actionsSection */}
        {selectedProviderId && (HAS_DASHBOARD.has(selectedProviderId) || HAS_STATUS_PAGE.has(selectedProviderId)) && (
          <div className="context-actions">
            <div className="context-actions__divider" />
            {HAS_DASHBOARD.has(selectedProviderId) && (
              <button
                type="button"
                className="context-actions__btn"
                onClick={() => void openProviderDashboard(selectedProviderId)}
              >
                <span className="context-actions__icon" aria-hidden>
                  <svg width="13" height="13" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">
                    <rect x="2" y="9" width="2.5" height="5" rx="0.6" fill="currentColor" />
                    <rect x="6.75" y="6" width="2.5" height="8" rx="0.6" fill="currentColor" />
                    <rect x="11.5" y="3" width="2.5" height="11" rx="0.6" fill="currentColor" />
                  </svg>
                </span>
                {t("ActionUsageDashboard")}
              </button>
            )}
            {HAS_STATUS_PAGE.has(selectedProviderId) && (
              <button
                type="button"
                className="context-actions__btn"
                onClick={() => void openProviderStatusPage(selectedProviderId)}
              >
                <span className="context-actions__icon" aria-hidden>
                  <svg width="14" height="13" viewBox="0 0 18 14" fill="none" xmlns="http://www.w3.org/2000/svg">
                    <path d="M1 7H4L5.5 3L8 11L10.5 5L12 7H17" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" fill="none" />
                  </svg>
                </span>
                {t("ActionStatusPage")}
              </button>
            )}
          </div>
        )}
      </MenuSurface>
      <TrayResizeHandles />
    </div>
  );
}

/**
 * Invisible resize grips along the flyout's in-screen edges (top / left /
 * top-left corner). The flyout is anchored bottom-right above the tray, so these
 * let the user widen (left edge) or heighten (top edge) it. Native edge-resize
 * doesn't work through the borderless WebView2, so we drive it explicitly with
 * `startResizeDragging`. That call enters a Win32 modal size loop which
 * transiently steals focus from the WebView2 child for its duration — Windows
 * fires a spurious `Focused(false)` the instant the press starts even though
 * the user never left the window. We arm a gesture-scoped blur guard on the
 * backend *before* starting the loop so that transient blur doesn't
 * auto-hide the flyout; the guard clears itself once focus genuinely returns
 * (via the `Focused(true)` refocus path) or after a 15s expiry, so no
 * explicit end call is needed here — the OS loop swallows mouseup.
 */
function TrayResizeHandles() {
  return (
    <>
      <div
        className="tray-resize tray-resize--top"
        aria-hidden
        onMouseDown={(e) => {
          e.preventDefault();
          void (async () => {
            await beginFlyoutGesture().catch(() => {});
            await getCurrentWindow().startResizeDragging("North");
          })().catch((err) => console.error("[tray-resize] startResizeDragging failed:", err));
        }}
      />
      <div
        className="tray-resize tray-resize--left"
        aria-hidden
        onMouseDown={(e) => {
          e.preventDefault();
          void (async () => {
            await beginFlyoutGesture().catch(() => {});
            await getCurrentWindow().startResizeDragging("West");
          })().catch((err) => console.error("[tray-resize] startResizeDragging failed:", err));
        }}
      />
      <div
        className="tray-resize tray-resize--topleft"
        aria-hidden
        onMouseDown={(e) => {
          e.preventDefault();
          void (async () => {
            await beginFlyoutGesture().catch(() => {});
            await getCurrentWindow().startResizeDragging("NorthWest");
          })().catch((err) => console.error("[tray-resize] startResizeDragging failed:", err));
        }}
      />
    </>
  );
}
