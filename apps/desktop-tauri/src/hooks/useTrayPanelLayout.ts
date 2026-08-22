import { useCallback, useEffect, useRef, useState } from "react";
import {
  getCurrentWindow,
  LogicalSize,
  PhysicalSize,
} from "@tauri-apps/api/window";
import {
  getWorkAreaRect,
  reanchorTrayPanel,
  revealTrayPanelWindow,
} from "../lib/tauri";
import {
  decideTrayHeight,
  EMPTY_AUTOFIT_STATE,
  recordAutoFitCommit,
  type TrayAutoFitState,
} from "../lib/traySizing";

const TRAY_WIDTH = 328;
const TRAY_MAX_MEASURE_HEIGHT = 920;
const TRAY_OVERVIEW_MIN_HEIGHT = 200;
const TRAY_DETAIL_MIN_HEIGHT = 420;
const TRAY_DENSE_OVERVIEW_HEIGHT = 776;

/** Scale a measured content height into the window's pixel space. TrayPanel
 * renders the surface with `zoom: trayScale` (CSS zoom), but every DOM metric
 * the layout pass reads — scrollHeight AND bounding rects — is reported in
 * the surface's LOCAL, pre-zoom px (measured identical at 100/150/200%). The
 * WIN32 window must be sized in POST-zoom px or tall cards clip below the
 * fold (and the zoomed width overflows into a horizontal scrollbar), so the
 * raw measure is multiplied by the active zoom before clamping (#265). */
export function scaledContentHeight(rawHeight: number, zoom: number): number {
  if (!Number.isFinite(zoom) || zoom <= 0 || zoom === 1) return rawHeight;
  return Math.round(rawHeight * zoom);
}

export interface TrayPanelLayoutOptions {
  canMeasure: boolean;
  denseOverview: boolean;
  detailMode: boolean;
  layoutKey: string;
  /** Auto-fit the window to its content (until the user sets a size). */
  autoFit?: boolean;
  /** The user's remembered size, re-applied + re-anchored each time the flyout
   *  opens. `null` when the user has not resized yet. */
  fixedSize?: [number, number] | null;
  /** Whether the flyout is currently open (surface mode === trayPanel). Used as
   *  the "just opened" trigger for the fixed-size restore + re-anchor. */
  isOpen?: boolean;
  /** Active tray-panel zoom factor (the CSS `zoom` TrayPanel applies to the
   *  surface). Auto-fit scales its measured content height by this before
   *  clamping — see `scaledContentHeight`. Defaults to 1 (no zoom). */
  zoom?: number;
  /** Called with the new logical size on a genuine user drag-resize. */
  onUserResize?: (width: number, height: number) => void;
}

export interface TrayPanelLayout {
  layoutReady: boolean;
  requestLayout: () => void;
}

export function useTrayPanelLayout({
  canMeasure,
  denseOverview,
  detailMode,
  layoutKey,
  autoFit = true,
  fixedSize = null,
  isOpen = false,
  zoom = 1,
  onUserResize,
}: TrayPanelLayoutOptions): TrayPanelLayout {
  const [layoutReady, setLayoutReady] = useState(false);
  const [layoutRevision, setLayoutRevision] = useState(0);
  const layoutReadyRef = useRef(false);
  const resizeRunRef = useRef(0);
  const layoutTimerRef = useRef<number | undefined>(undefined);
  // The window's actual PHYSICAL size after the last resize WE performed. The
  // onResized event also reports physical pixels, so comparing physical-to-
  // physical needs no scale factor — Tauri scaleFactor / webview devicePixelRatio
  // / Win32 can all disagree on a scaled display, and that disagreement is what
  // compounded a per-open size growth.
  const lastSizeRef = useRef<{ width: number; height: number } | null>(null);
  const programmaticInFlightRef = useRef(0);
  // Auto-fit sizing decision state (committed frame + one-frame history +
  // learned oscillation pair) — all frame logic lives in lib/traySizing so
  // the #261 cycle detection stays pure and directly testable.
  const sizingStateRef = useRef<TrayAutoFitState>(EMPTY_AUTOFIT_STATE);
  const fixedSizeRef = useRef(fixedSize);
  useEffect(() => {
    fixedSizeRef.current = fixedSize;
  }, [fixedSize]);
  const onUserResizeRef = useRef(onUserResize);
  useEffect(() => {
    onUserResizeRef.current = onUserResize;
  }, [onUserResize]);

  // Resize the window and record the resulting ACTUAL physical size. Wrapped in
  // an in-flight counter so the Resized event(s) this triggers can never be
  // mistaken for a user drag, regardless of event timing.
  const applySize = useCallback(
    async (size: LogicalSize | PhysicalSize): Promise<void> => {
      const win = getCurrentWindow();
      programmaticInFlightRef.current += 1;
      try {
        await win.setSize(size);
        const actual = await win.innerSize();
        lastSizeRef.current = { width: actual.width, height: actual.height };
      } catch {
        /* ignore */
      } finally {
        programmaticInFlightRef.current -= 1;
      }
    },
    [],
  );

  // Report genuine user drag-resizes. Ignore resizes that fire while WE are
  // resizing (in-flight counter) or whose physical size still matches the last
  // size we applied; anything else is the user dragging the border. Everything
  // is in PHYSICAL pixels — no scale conversion, so it can't drift.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    const win = getCurrentWindow();
    void (async () => {
      try {
        unlisten = await win.onResized(({ payload }) => {
          if (programmaticInFlightRef.current > 0) return;
          const last = lastSizeRef.current;
          if (
            last &&
            Math.abs(payload.width - last.width) <= 3 &&
            Math.abs(payload.height - last.height) <= 3
          ) {
            return;
          }
          onUserResizeRef.current?.(payload.width, payload.height);
        });
      } catch {
        unlisten = undefined;
      }
      if (cancelled) unlisten?.();
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // User-sized flyout: on each open, apply the remembered size, re-anchor above
  // the tray at THAT size (so the anchor math uses the real height, not the
  // default), then reveal. Content scrolls inside the fixed window via CSS.
  const hasFixedSize = fixedSize != null;
  useEffect(() => {
    if (autoFit || !isOpen || !canMeasure) return;
    const fixed = fixedSizeRef.current;
    if (!fixed) return;
    let cancelled = false;
    void (async () => {
      // `fixed` is the user's remembered PHYSICAL size (scale-independent).
      await applySize(new PhysicalSize(fixed[0], fixed[1]));
      await Promise.resolve(reanchorTrayPanel()).catch(() => {});
      if (cancelled) return;
      layoutReadyRef.current = true;
      setLayoutReady(true);
      await Promise.resolve(revealTrayPanelWindow()).catch(() => {});
    })();
    return () => {
      cancelled = true;
    };
  }, [autoFit, isOpen, canMeasure, hasFixedSize, applySize]);

  const requestLayout = useCallback(() => {
    if (layoutTimerRef.current !== undefined) {
      window.clearTimeout(layoutTimerRef.current);
    }
    layoutTimerRef.current = window.setTimeout(() => {
      setLayoutRevision((current) => current + 1);
    }, layoutReadyRef.current ? 100 : 16);
  }, []);

  useEffect(() => {
    requestLayout();
  }, [layoutKey, requestLayout]);

  useEffect(() => {
    const surface = document.querySelector<HTMLElement>(".menu-surface--tray");
    if (!surface || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      // Measuring temporarily removes the surface/body constraints, which
      // resizes the observed surface. Do not feed that programmatic change
      // back into another pass or the capped flyout flashes between its
      // measured and committed layouts forever.
      if (
        layoutReadyRef.current &&
        programmaticInFlightRef.current > 0
      ) {
        return;
      }
      requestLayout();
    });
    observer.observe(surface);
    return () => observer.disconnect();
  }, [requestLayout]);

  useEffect(() => {
    return () => {
      if (layoutTimerRef.current !== undefined) {
        window.clearTimeout(layoutTimerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (!autoFit || !canMeasure) return;

    const minHeight = detailMode
      ? TRAY_DETAIL_MIN_HEIGHT
      : denseOverview
        ? TRAY_DENSE_OVERVIEW_HEIGHT
        : TRAY_OVERVIEW_MIN_HEIGHT;

    const resize = async () => {
      const run = ++resizeRunRef.current;
      const surface = document.querySelector<HTMLElement>(".menu-surface--tray");
      if (!surface) return;
      const html = document.documentElement;
      const pageBody = document.body;
      const workArea = await getWorkAreaRect().catch(() => null);
      const maxHeight = Math.max(
        minHeight,
        Math.min(
          TRAY_MAX_MEASURE_HEIGHT,
          (workArea?.height ?? TRAY_MAX_MEASURE_HEIGHT) - 16,
        ),
      );

      const body = surface.querySelector<HTMLElement>(".menu-surface__body");
      const stack = surface.querySelector<HTMLElement>(".menu-stack");

      const previous = {
        htmlOverflow: html.style.overflow,
        bodyOverflow: pageBody.style.overflow,
        bodyMinHeight: pageBody.style.minHeight,
        surfaceMinHeight: surface.style.minHeight,
        surfaceHeight: surface.style.height,
        surfaceMaxHeight: surface.style.maxHeight,
        surfaceOverflow: surface.style.overflow,
        bodyInnerOverflow: body?.style.overflow,
        bodyFlex: body?.style.flex,
        stackOverflow: stack?.style.overflow,
      };
      let committedHeight = false;

      html.style.overflow = "visible";
      pageBody.style.overflow = "visible";
      pageBody.style.minHeight = "0";
      surface.style.minHeight = "0";
      surface.style.height = "auto";
      surface.style.maxHeight = "none";
      surface.style.overflow = "visible";
      if (body) {
        body.style.overflow = "visible";
        body.style.flex = "0 0 auto";
      }
      if (stack) {
        stack.style.overflow = "visible";
      }

      const revealPanel = async () => {
        if (run !== resizeRunRef.current) return;
        layoutReadyRef.current = true;
        setLayoutReady(true);
        await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
        if (run === resizeRunRef.current) {
          await Promise.resolve(revealTrayPanelWindow()).catch(() => {});
        }
      };

      // Suppress every Resized event this whole auto-fit pass causes (the burst
      // of setSize calls + any that arrive shortly after) so none is mistaken
      // for a user drag. The trailing delay absorbs late-delivered events.
      programmaticInFlightRef.current += 1;
      try {
        if (!layoutReadyRef.current) {
          sizingStateRef.current = recordAutoFitCommit(
            sizingStateRef.current,
            TRAY_WIDTH,
            minHeight,
            window.devicePixelRatio,
          );
          await applySize(new LogicalSize(TRAY_WIDTH, minHeight));
        }

        await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
        await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

        if (run !== resizeRunRef.current) return;

        const surfaceRect = surface.getBoundingClientRect();
        // Everything measured here is in the surface's LOCAL, pre-zoom px;
        // scale into post-zoom window px via `scaledContentHeight` at each
        // derivation point so the clamped window fits the rendered content.
        let contentHeight = scaledContentHeight(
          Math.max(surface.scrollHeight, Math.ceil(surfaceRect.height)),
          zoom,
        );
        let maxBottom = surfaceRect.top + contentHeight;
        const bodyRect = body?.getBoundingClientRect();
        if (bodyRect && bodyRect.height > 0) {
          const scaledBottom =
            surfaceRect.top +
            scaledContentHeight(bodyRect.bottom - surfaceRect.top, zoom);
          if (scaledBottom > maxBottom) {
            maxBottom = scaledBottom;
          }
        }
        const footer = surface.querySelector<HTMLElement>(".menu-surface__footer");
        const footerRect = footer?.getBoundingClientRect();
        if (footerRect && footerRect.height > 0) {
          const scaledBottom =
            surfaceRect.top +
            scaledContentHeight(footerRect.bottom - surfaceRect.top, zoom);
          if (scaledBottom > maxBottom) {
            maxBottom = scaledBottom;
          }
        }
        contentHeight = Math.ceil(maxBottom - surfaceRect.top) + 4;

        const height = Math.min(Math.max(contentHeight, minHeight), maxHeight);

        // #261: two-state cycle detection on physical targets. Normal rule
        // commits ANY real change (even +5 physical px); only exact same-
        // frame equality is a no-op, and a bounded A→B→A pair (span ≤8
        // physical, the reporter's 7) locks onto its larger member. The DOM
        // constraint is set AFTER the decision from the RETAINED height, so
        // surface and window never diverge.
        const decision = decideTrayHeight(
          {
            measuredHeight: height,
            expectedWidth: TRAY_WIDTH,
            minHeight,
            maxHeight,
            // WebView layout px ↔ Win32 physical px ratio; CSS zoom does not
            // affect it (zoom is already in `height` via scaledContentHeight).
            scaleFactor: window.devicePixelRatio,
            zoom,
            lastAppliedPhysicalHeight: lastSizeRef.current?.height ?? null,
          },
          sizingStateRef.current,
        );
        sizingStateRef.current = decision.state;
        surface.style.maxHeight = `${decision.height}px`;
        committedHeight = true;

        if (decision.commit) {
          await applySize(new LogicalSize(TRAY_WIDTH, decision.height));
          await Promise.resolve(reanchorTrayPanel()).catch(() => {});
        }

        await revealPanel();
      } catch (error) {
        console.warn("CodexBar tray panel resize failed", error);
        void revealPanel();
      } finally {
        if (!committedHeight) {
          surface.style.maxHeight = previous.surfaceMaxHeight;
        }
        surface.style.minHeight = previous.surfaceMinHeight;
        surface.style.height = previous.surfaceHeight;
        surface.style.overflow = previous.surfaceOverflow;
        html.style.overflow = previous.htmlOverflow;
        pageBody.style.overflow = previous.bodyOverflow;
        pageBody.style.minHeight = previous.bodyMinHeight;
        if (body) {
          body.style.overflow = previous.bodyInnerOverflow ?? "";
          body.style.flex = previous.bodyFlex ?? "";
        }
        if (stack) {
          stack.style.overflow = previous.stackOverflow ?? "";
        }
        window.setTimeout(() => {
          programmaticInFlightRef.current = Math.max(
            0,
            programmaticInFlightRef.current - 1,
          );
        }, 200);
      }
    };

    const timer = window.setTimeout(
      () => void resize(),
      layoutReadyRef.current ? 25 : 0,
    );

    return () => {
      window.clearTimeout(timer);
      resizeRunRef.current += 1;
    };
  }, [autoFit, canMeasure, denseOverview, detailMode, layoutRevision, applySize, zoom]);

  return { layoutReady, requestLayout };
}
