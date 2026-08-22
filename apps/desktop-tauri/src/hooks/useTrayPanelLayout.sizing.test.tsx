import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TrayPanelLayoutOptions } from "./useTrayPanelLayout";

// #261 hook-level proof of the two-state cycle detection:
//  - stable one-way +5-physical-px changes COMMIT (no blanket deadband);
//  - a bounded 679↔686-physical pair (the reporter's 7-px amplitude) is
//    detected once, converges to the LARGER member, and stops committing;
//  - during suppression the surface's max-height tracks the RETAINED window
//    target (never the freshly measured, smaller candidate → no clipping);
//  - genuine growth/shrink outside the pair clears and commits;
//  - anchors fire exactly on real commits (bottom-anchored flow intact).
//
// Sequencing instead of sleeps: every completed pass marks the surface's
// max-height (it is set on EVERY pass from the decision, suppressed or not),
// so each nudge waits for its own marker value.
const SCALE = 1.25;

const tauriMocks = vi.hoisted(() => ({
  getWorkAreaRect: vi
    .fn()
    .mockResolvedValue({ x: 0, y: 0, width: 1280, height: 900 }),
  reanchorTrayPanel: vi.fn().mockResolvedValue(undefined),
  revealTrayPanelWindow: vi.fn().mockResolvedValue(undefined),
}));

const windowMocks = vi.hoisted(() => ({
  setSize: vi.fn().mockResolvedValue(undefined),
  innerSize: vi.fn().mockResolvedValue({ width: 328, height: 420 }),
  getCurrentWindow: vi.fn(),
  LogicalSize: vi.fn((width: number, height: number) => ({ width, height })),
  PhysicalSize: vi.fn((width: number, height: number) => ({ width, height })),
}));

vi.mock("../lib/tauri", () => tauriMocks);
vi.mock("@tauri-apps/api/window", () => windowMocks);

import { useTrayPanelLayout } from "./useTrayPanelLayout";

let surface: HTMLElement;
let feedbackObserverCallbacks = 0;

class StyleFeedbackResizeObserver {
  private mutationObserver: MutationObserver | null = null;

  constructor(private readonly callback: ResizeObserverCallback) {}

  observe(target: Element): void {
    this.mutationObserver = new MutationObserver(() => {
      feedbackObserverCallbacks += 1;
      this.callback([], this as unknown as ResizeObserver);
    });
    this.mutationObserver.observe(target, {
      attributes: true,
      attributeFilter: ["style"],
    });
  }

  unobserve(): void {
    this.disconnect();
  }

  disconnect(): void {
    this.mutationObserver?.disconnect();
    this.mutationObserver = null;
  }
}

function mountSurface(): void {
  document.body.innerHTML = [
    '<div class="menu-surface menu-surface--tray">',
    '<div class="menu-surface__body">',
    '<div class="menu-stack"></div>',
    "</div>",
    '<nav class="menu-surface__footer"></nav>',
    "</div>",
  ].join("");
  surface = document.querySelector<HTMLElement>(".menu-surface--tray")!;
}

/** Drive the auto-fit measure: jsdom rects are 0, so scrollHeight dominates →
 *  contentHeight = scrollHeight + 4 (measure pipeline, zoom=1). */
function setScrollHeight(px: number): void {
  Object.defineProperty(surface, "scrollHeight", {
    configurable: true,
    get: () => px,
  });
}

function hookProps(overrides: Partial<TrayPanelLayoutOptions> = {}): TrayPanelLayoutOptions {
  return {
    canMeasure: true,
    denseOverview: false,
    detailMode: true,
    layoutKey: "sizing",
    ...overrides,
  };
}

function lastResize(): { width: number; height: number } {
  const calls = windowMocks.setSize.mock.calls;
  return calls[calls.length - 1][0];
}

function hookResult(result: unknown): { current: { requestLayout: () => void } } {
  return result as { current: { requestLayout: () => void } };
}

describe("useTrayPanelLayout sizing", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    feedbackObserverCallbacks = 0;
    mountSurface();
    Object.defineProperty(window, "devicePixelRatio", {
      configurable: true,
      value: SCALE,
    });
    // Win32 applies integer-physical sizes: readback = round(logical * 1.25).
    windowMocks.setSize.mockImplementation(
      async (size: { width: number; height: number }) => {
        windowMocks.innerSize.mockResolvedValue({
          width: Math.round(size.width * SCALE),
          height: Math.round(size.height * SCALE),
        });
      },
    );
    windowMocks.getCurrentWindow.mockReturnValue({
      setSize: windowMocks.setSize,
      close: vi.fn().mockResolvedValue(undefined),
      scaleFactor: vi.fn().mockResolvedValue(SCALE),
      onResized: vi.fn().mockResolvedValue(() => {}),
      innerSize: windowMocks.innerSize,
    } as never);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
  });

  /** Nudge a pass and wait until IT marks the surface with `expectedMarker`
   *  (maxHeight is assigned on every pass → deterministic per-pass signal,
   *  immune to nudge-vs-pass alignment races). Returns nothing; callers assert
   *  setSize/anchor deltas against counts they snapshotted before the nudge. */
  async function nudgePass(
    result: unknown,
    sh: number,
    expectedMarker: string,
  ): Promise<void> {
    const r = hookResult(result);
    const alreadyMarked = surface.style.maxHeight === expectedMarker;
    const revealsAtNudge = tauriMocks.revealTrayPanelWindow.mock.calls.length;
    setScrollHeight(sh);
    r.current.requestLayout();
    await waitFor(
      () =>
        alreadyMarked
          ? expect(
              tauriMocks.revealTrayPanelWindow.mock.calls.length,
            ).toBeGreaterThan(revealsAtNudge)
          : expect(surface.style.maxHeight).toBe(expectedMarker),
      { timeout: 3000 },
    );
  }

  it("does not feed measurement style changes back into another auto-fit pass", async () => {
    vi.stubGlobal("ResizeObserver", StyleFeedbackResizeObserver);
    setScrollHeight(1_200);

    const { result } = renderHook(() => useTrayPanelLayout(hookProps()));
    await waitFor(() => expect(result.current.layoutReady).toBe(true), {
      timeout: 3000,
    });
    await waitFor(() => expect(feedbackObserverCallbacks).toBeGreaterThan(0));

    await act(async () => {
      await new Promise((resolve) => window.setTimeout(resolve, 300));
    });
    const settledRevealCount =
      tauriMocks.revealTrayPanelWindow.mock.calls.length;
    await act(async () => {
      await new Promise((resolve) => window.setTimeout(resolve, 500));
    });

    expect(tauriMocks.revealTrayPanelWindow.mock.calls.length).toBe(
      settledRevealCount,
    );
  });

  it("commits stable small changes, locks the reporter pair on the larger member, tracks retained height in the DOM", async () => {
    setScrollHeight(535); // → 539 logical → 674 physical
    const { result } = renderHook(() => useTrayPanelLayout(hookProps()));
    await waitFor(() => expect(result.current.layoutReady).toBe(true), {
      timeout: 3000,
    });
    expect(
      windowMocks.setSize.mock.calls.some(
        (call) => (call[0] as { height: number }).height === 539,
      ),
    ).toBe(true);

    // (1) Stable one-way +5-physical change COMMITS (no blanket deadband).
    await nudgePass(result, 539, "543px"); // → 543 → 679 phys
    expect(lastResize()).toEqual({ width: 328, height: 543 });
    expect(surface.style.maxHeight).toBe("543px");

    // (2) Reporter-class alternation: one commit to 549 (686 phys), then the
    // 543↔549 pair (679↔686 phys, span 7) is detected on the flip-down.
    await nudgePass(result, 545, "549px"); // → 549 → 686 phys
    expect(lastResize()).toEqual({ width: 328, height: 549 });
    expect(surface.style.maxHeight).toBe("549px");
    const lockedResizes = windowMocks.setSize.mock.calls.length;
    const lockedAnchors = tauriMocks.reanchorTrayPanel.mock.calls.length;

    // Flip-down evidence (measure 539→543): detected, suppressed, and the DOM
    // constraint stays the RETAINED height — never the smaller candidate.
    await nudgePass(result, 539, "549px");
    expect(windowMocks.setSize.mock.calls.length).toBe(lockedResizes);
    expect(tauriMocks.reanchorTrayPanel.mock.calls.length).toBe(lockedAnchors);
    expect(surface.style.maxHeight).toBe("549px");

    // (3) Repeated flips stay suppressed; surface stays at the retained 549.
    await nudgePass(result, 545, "549px");
    await nudgePass(result, 539, "549px");
    expect(windowMocks.setSize.mock.calls.length).toBe(lockedResizes);
    expect(tauriMocks.reanchorTrayPanel.mock.calls.length).toBe(lockedAnchors);
    expect(surface.style.maxHeight).toBe("549px");
    // Retained window (549 logical) fully contains BOTH measured sides
    // (543 and 549 candidates) — no clipping by construction.
    expect(lastResize()).toEqual({ width: 328, height: 549 });

    // (4) Real growth outside the pair clears the lock and commits once.
    await nudgePass(result, 700, "704px"); // → 704 → 880 phys
    expect(lastResize()).toEqual({ width: 328, height: 704 });
    expect(surface.style.maxHeight).toBe("704px");

    // Real shrink commits once.
    await nudgePass(result, 410, "420px"); // → clamp 420 → 525 phys
    expect(lastResize()).toEqual({ width: 328, height: 420 });
    expect(surface.style.maxHeight).toBe("420px");

    // (5) No blanket absorption: a stable 1-physical-px change still commits.
    await nudgePass(result, 417, "421px"); // → 421 → 526 phys
    expect(lastResize()).toEqual({ width: 328, height: 421 });
    expect(surface.style.maxHeight).toBe("421px");
  });

  it("reconciles to the applied physical frame after an OS snap (no churn, no cycle)", async () => {
    // Deliberate 5-physical snap: requesting 539 logical (→674 phys) yields an
    // innerSize readback of 669.
    windowMocks.setSize.mockImplementation(
      async (size: { width: number; height: number }) => {
        windowMocks.innerSize.mockResolvedValue({
          width: Math.round(size.width * SCALE),
          height: size.height === 539 ? 669 : Math.round(size.height * SCALE),
        });
      },
    );

    setScrollHeight(535); // → 539 target (674 phys requested) → applied 669
    const { result } = renderHook(() => useTrayPanelLayout(hookProps()));
    await waitFor(() => expect(result.current.layoutReady).toBe(true), {
      timeout: 3000,
    });
    expect(
      windowMocks.setSize.mock.calls.some(
        (call) => (call[0] as { height: number }).height === 539,
      ),
    ).toBe(true);
    const snappedResizes = windowMocks.setSize.mock.calls.length;
    const snappedAnchors = tauriMocks.reanchorTrayPanel.mock.calls.length;

    // Candidate 535 (→669 phys) equals the APPLIED frame while the recorded
    // target says 674: suppress (no setSize/reanchor), adopt the candidate.
    // The prior frame is 420 (525 phys), 144 px away from 669 — the A↔B
    // detector cannot fire here.
    await nudgePass(result, 531, "535px"); // → 535 → 669 phys
    expect(windowMocks.setSize.mock.calls.length).toBe(snappedResizes);
    expect(tauriMocks.reanchorTrayPanel.mock.calls.length).toBe(snappedAnchors);
    expect(surface.style.maxHeight).toBe("535px");

    // Identical next pass: now exact same-frame stable — still zero churn.
    await nudgePass(result, 531, "535px");
    expect(windowMocks.setSize.mock.calls.length).toBe(snappedResizes);
    expect(tauriMocks.reanchorTrayPanel.mock.calls.length).toBe(snappedAnchors);
    expect(surface.style.maxHeight).toBe("535px");

    // Recovery: a real +5-physical change from the reconciled frame commits.
    await nudgePass(result, 535, "539px"); // → 539 → 674 phys
    expect(lastResize()).toEqual({ width: 328, height: 539 });
    expect(surface.style.maxHeight).toBe("539px");
  });
});
