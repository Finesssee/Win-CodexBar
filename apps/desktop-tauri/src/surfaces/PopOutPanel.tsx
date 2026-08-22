import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { BootstrapState, ProviderUsageSnapshot } from "../types/bridge";
import { openFlyoutWindow, openSettingsWindow, quitApp as quitApplication, reorderProviders } from "../lib/tauri";
import { useProviders } from "../hooks/useProviders";
import { useSettings } from "../hooks/useSettings";
import { useUpdateState } from "../hooks/useUpdateState";
import { useLocale } from "../hooks/useLocale";
import MenuCard from "../components/MenuCard";
import PopOutTitleBar from "../components/PopOutTitleBar";
import MenuSurface, {
  MenuEmpty,
  type MenuFooterRow,
} from "../components/MenuSurface";
import UpdateBanner from "../components/UpdateBanner";
import ProviderGrid from "../components/ProviderGrid";
import { orderProviderSnapshots } from "../lib/providerOrder";

/**
 * Pop-out window — dashboard and provider deep-links both keep the full card
 * stack. A provider target only scrolls/focuses the requested card so the
 * layout stays consistent with the tray/menu surface.
 */
export default function PopOutPanel({
  state,
  providerId,
}: {
  state: BootstrapState;
  providerId?: string;
}) {
  const {
    providers,
    isRefreshing,
    refreshingProviderIds,
    refresh,
    hasCachedData,
  } = useProviders();
  const { settings } = useSettings(state.settings);
  const { updateState, checkNow, download, apply, dismiss, openRelease } =
    useUpdateState();
  const { t } = useLocale();

  const sorted = useMemo(() => {
    return orderProviderSnapshots(
      providers,
      state.providers,
      settings.enabledProviders,
      settings.providerOrder,
    );
  }, [providers, settings.enabledProviders, settings.providerOrder, state.providers]);
  const [selectedProviderId, setSelectedProviderId] = useState<string | null>(
    providerId ?? null,
  );
  const [gridExpanded, setGridExpanded] = useState(false);
  const cardRefs = useRef(new Map<string, HTMLDivElement>());
  const windowScale = useMemo(() => {
    const scalePercent = Number(settings.windowScalePercent);
    return (
      Math.min(250, Math.max(100, Number.isFinite(scalePercent) ? scalePercent : 100)) / 100
    );
  }, [settings.windowScalePercent]);

  // Scale the dashboard via the webview's native zoom (like a browser's Ctrl-+):
  // it reflows content at the real window width, so the side-by-side cards keep
  // filling the window at any scale — unlike CSS `zoom`, which overflows. The
  // main window is shared with the tray surface, so reset zoom to 1 on unmount.
  useEffect(() => {
    const webview = getCurrentWebviewWindow();
    void webview.setZoom(windowScale).catch(() => {});
    return () => {
      void webview.setZoom(1).catch(() => {});
    };
  }, [windowScale]);

  useEffect(() => {
    setSelectedProviderId(providerId ?? null);
  }, [providerId]);

  const visibleProviders = useMemo(
    () => {
      if (selectedProviderId === null) {
        if (sorted.length + 1 > 32 && !gridExpanded) {
          return sorted.slice(0, 4);
        }
        return sorted;
      }
      const match = sorted.find((p) => p.providerId === selectedProviderId);
      return match ? [match] : sorted;
    },
    [sorted, selectedProviderId, gridExpanded],
  );
  const providerOrderKey = useMemo(
    () => sorted.map((provider) => provider.providerId).join(","),
    [sorted],
  );

  const handleGridClick = useCallback((nextProviderId: string | null) => {
    setSelectedProviderId(nextProviderId);
  }, []);
  const handleReorder = useCallback((orderedIds: string[]) => {
    void reorderProviders(orderedIds).catch(() => {});
  }, []);

  useEffect(() => {
    if (!providerId || selectedProviderId !== providerId || providerOrderKey.length === 0) return;

    let cancelled = false;
    const scrollToProvider = () => {
      if (cancelled) return;
      const target = cardRefs.current.get(providerId);
      if (!target) return;

      window.scrollTo(0, 0);
      if (document.scrollingElement) {
        document.scrollingElement.scrollTop = 0;
      }
      document.documentElement.scrollTop = 0;
      document.body.scrollTop = 0;

      for (const selector of [".menu-stack", ".menu-surface__body"]) {
        const container = target.closest<HTMLElement>(selector);
        if (!container) continue;
        container.scrollTop = 0;
        const targetRect = target.getBoundingClientRect();
        const containerRect = container.getBoundingClientRect();
        container.scrollTop += targetRect.top - containerRect.top;
      }
    };

    requestAnimationFrame(() => {
      requestAnimationFrame(scrollToProvider);
    });
    const timer = window.setTimeout(scrollToProvider, 100);
    const lateTimer = window.setTimeout(scrollToProvider, 350);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      window.clearTimeout(lateTimer);
    };
  }, [providerId, selectedProviderId, providerOrderKey]);

  const openSettings = useCallback(() => {
    openSettingsWindow("general");
  }, []);
  const goTray = useCallback(() => {
    // The flyout ("Pop Out Dashboard") is now its own dedicated OS window
    // rather than a state of the shared `main` window's surface-mode
    // machine, so "back to tray" opens it directly instead of switching
    // `main`'s mode.
    void openFlyoutWindow().catch(() => {});
  }, []);
  const openAbout = useCallback(() => {
    openSettingsWindow("about");
  }, []);
  const quitApp = useCallback(() => {
    void quitApplication();
  }, []);

  const headerActions = [
    { icon: "⊟", title: t("TooltipBackToTray"), onClick: goTray },
  ];

  const footerRows: MenuFooterRow[] = [
    { icon: "⚙", label: t("TooltipSettings"), shortcut: "Ctrl+,", onClick: openSettings },
    { icon: "ℹ", label: t("MenuAbout"), onClick: openAbout },
    { icon: "✕", label: t("MenuQuit"), shortcut: "Ctrl+Q", onClick: quitApp },
  ];

  // Keyboard shortcuts
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
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

  const surface = sorted.length === 0 ? (
    <MenuSurface
      variant="popout"
      titleBar={<PopOutTitleBar />}
      onRefresh={refresh}
      isRefreshing={isRefreshing}
      actions={headerActions}
      banner={banner}
      footerRows={footerRows}
    >
      <MenuEmpty
        isLoading={isRefreshing && !hasCachedData}
        onSettings={openSettings}
      />
    </MenuSurface>
  ) : (
    <MenuSurface
      variant="popout"
      titleBar={<PopOutTitleBar />}
      onRefresh={refresh}
      isRefreshing={isRefreshing}
      actions={headerActions}
      banner={banner}
      footerRows={footerRows}
    >
      <ProviderGrid
        providers={sorted}
        selectedProviderId={selectedProviderId}
        showAsUsed={settings.showAsUsed}
        showProviderIcons={settings.switcherShowsIcons}
        expanded={gridExpanded}
        onExpandedChange={setGridExpanded}
        onSelect={handleGridClick}
        onReorder={handleReorder}
      />
      <div className="provider-grid__divider" />
      <div className="menu-stack">
        {visibleProviders.map((p, idx) => (
          <Fragment key={p.providerId}>
            {idx > 0 && <div className="menu-stack__sep" />}
            <div
              className={`menu-stack__item${selectedProviderId === p.providerId ? " menu-stack__item--selected" : ""}`}
              ref={(node) => {
                if (node) {
                  cardRefs.current.set(p.providerId, node);
                } else {
                  cardRefs.current.delete(p.providerId);
                }
              }}
            >
              <MenuCard
                provider={p}
                isRefreshing={refreshingProviderIds.has(p.providerId)}
                display={{
                  hideEmail: settings.hidePersonalInfo,
                  resetTimeRelative: settings.resetTimeRelative,
                  showResetWhenExhausted: settings.showResetWhenExhausted,
            showPace: settings.showPace ?? true,
                  showAsUsed: settings.showAsUsed,
                  compactMetrics: selectedProviderId === null,
                  costSummaryDisplayStyle: settings.costSummaryDisplayStyle,
                }}
                accentColor={settings.providerAccentColors[p.providerId]}
              />
            </div>
          </Fragment>
        ))}
      </div>
    </MenuSurface>
  );

  return (
    <div className="popout-scale-shell">
      {surface}
    </div>
  );
}
