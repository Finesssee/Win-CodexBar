import { Fragment, type CSSProperties } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { BootstrapState, ProviderUsageSnapshot } from "../types/bridge";
import { beginFlyoutGesture, openProviderDashboard, openProviderStatusPage } from "../lib/tauri";
import {
  TRAY_SCALE_MAX,
  TRAY_SCALE_MIN,
  TRAY_SCALE_STEP,
  useTrayPanelController,
} from "../hooks/useTrayPanelController";
import MenuCard from "../components/MenuCard";
import MenuSurface, { MenuEmpty } from "../components/MenuSurface";
import UpdateBanner from "../components/UpdateBanner";
import ProviderGrid from "../components/ProviderGrid";
import AgentSessions from "../components/AgentSessions";

/** Provider IDs that have a dashboard URL in the backend */
const HAS_DASHBOARD = new Set([
  "abacus", "alibaba", "alibabatokenplan", "amp", "augment",
  "azureopenai", "bedrock", "claude", "codex", "codebuff",
  "aiand", "commandcode", "copilot", "crof", "crossmodel", "cursor", "deepgram", "deepinfra", "deepseek", "zenmux", "clinepass", "longcat", "neuralwatt", "zoommate",
  "doubao", "elevenlabs", "factory", "gemini", "grok", "groq",
  "infini", "jetbrains", "kilo", "kimi", "kimik2", "kiro", "manus",
  "mimo", "minimax", "mistral", "nanogpt", "notion", "ollama", "openaiapi",
  "opencode", "opencodego", "openrouter", "perplexity", "qoder", "codebuddy", "sakana", "stepfun",
  "t3chat", "venice", "vertexai", "warp", "windsurf",
  "xai", "zai",
]);
/** Provider IDs that have a status page URL in the backend */
const HAS_STATUS_PAGE = new Set([
  "alibabatokenplan", "amp", "augment", "azureopenai", "bedrock",
  "claude", "codex", "copilot", "deepgram", "deepinfra", "deepseek", "zenmux", "clinepass", "longcat", "neuralwatt", "zoommate", "elevenlabs",
  "gemini", "grok", "groq", "kiro", "mistral", "openaiapi",
  "openrouter", "vertexai", "windsurf", "xai",
]);

/**
 * Tray popover surface — two modes like macOS CodexBar:
 * 1. Overview (default): provider grid + all cards stacked
 * 2. Detail: click a provider in grid → show only that provider's card
 */
export default function TrayPanel({ state }: { state: BootstrapState }) {
  const {
    t,
    settings,
    isRefreshing,
    refreshingProviderIds,
    refresh,
    hasCachedData,
    trayScaleDraft,
    trayScale,
    trayScaleFillPercent,
    handleTrayScaleChange,
    sorted,
    denseTrayProviders,
    expectsDenseOverview,
    selectedProviderId,
    gridExpanded,
    setGridExpanded,
    visibleProviders,
    wideColumns,
    useWideColumns,
    requestLayout,
    headerActions,
    footerRows,
    updateState,
    checkNow,
    download,
    apply,
    dismiss,
    openRelease,
    openSettings,
    handleGridClick,
    handleReorder,
    handleGestureStart,
    handleGestureEnd,
    revealClassName,
  } = useTrayPanelController(state);

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
          display={{
            hideEmail: settings.hidePersonalInfo,
            resetTimeRelative: settings.resetTimeRelative,
            showResetWhenExhausted: settings.showResetWhenExhausted,
            showAsUsed: settings.showAsUsed,
            compactMetrics: selectedProviderId === null,
          }}
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
            ? wideColumns.map((column) => (
                <div
                  className="menu-stack__column"
                  key={column.map((p) => p.providerId).join("|") || "empty"}
                >
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
