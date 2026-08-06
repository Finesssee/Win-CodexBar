import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
import { useProviders } from "./useProviders";
import { useSettings } from "./useSettings";
import { useUpdateState } from "./useUpdateState";
import { useLocale } from "./useLocale";
import { useSurfaceTarget } from "./useSurfaceMode";
import { useTrayPanelLayout } from "./useTrayPanelLayout";
import type { MenuFooterRow } from "../components/MenuSurface";
import { orderProviderSnapshots } from "../lib/providerOrder";
import {
  hydrateProviderSlots,
  orderedEnabledProviderSlots,
} from "../lib/trayProviders";

const TRAY_INITIAL_REFRESH_DELAY_MS = 250;
const DENSE_OVERVIEW_THRESHOLD = 32;

// ── Tray flyout zoom (footer slider, above Refresh) ───────────────────
// PopOut window mode has its own independent windowScalePercent (webview
// setZoom) — this is a separate setting/control for the tray flyout only,
// applied via CSS `zoom` on the MenuSurface root (see TrayPanel render).
export const TRAY_SCALE_MIN = 100;
export const TRAY_SCALE_MAX = 200;
export const TRAY_SCALE_STEP = 5;
const TRAY_SCALE_COMMIT_DEBOUNCE_MS = 250;

function clampTrayScalePercent(value: number): number {
  return Math.min(
    TRAY_SCALE_MAX,
    Math.max(TRAY_SCALE_MIN, Number.isFinite(value) ? value : 100),
  );
}

/**
 * Controller for the tray flyout surface — state, memos, effects, and
 * handlers. JSX stays in `TrayPanel`.
 */
export function useTrayPanelController(state: BootstrapState) {
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
        return denseTrayProviders.slice(0, 4);
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
    zoom: trayScale,
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

  const revealClassName = `tray-panel-reveal${layoutReady ? " tray-panel-reveal--ready" : ""}${expectsDenseOverview ? " tray-panel-reveal--dense" : ""}${fixedFlyoutSize ? " tray-panel-reveal--usersized" : ""}`;

  return {
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
    layoutReady,
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
  };
}
