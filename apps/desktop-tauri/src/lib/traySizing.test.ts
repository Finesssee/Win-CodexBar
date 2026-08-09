import { describe, expect, it } from "vitest";
import {
  decideTrayHeight,
  EMPTY_AUTOFIT_STATE,
  recordAutoFitCommit,
  TRAY_CYCLE_SPAN_MAX_PHYSICAL_PX,
  type TrayAutoFitState,
  type TraySizingInput,
} from "./traySizing";

const TRAY_WIDTH = 328;
const MIN = 420;
const MAX = 920;

function input(overrides: Partial<TraySizingInput> = {}): TraySizingInput {
  return {
    measuredHeight: 500,
    expectedWidth: TRAY_WIDTH,
    minHeight: MIN,
    maxHeight: MAX,
    scaleFactor: 1,
    zoom: 1,
    lastAppliedPhysicalHeight: null,
    ...overrides,
  };
}

interface ReplayStep {
  measured: number;
  reason: string;
  commit: boolean;
  height: number;
}

/** Replay a candidate stream the way the hook does: a commit updates the
 *  returned state AND the applied-physical readback (Win32 applies
 *  round(height*sf)); a suppression changes neither. */
function replay(
  scaleFactor: number,
  candidates: number[],
  overrides: Partial<TraySizingInput> = {},
): { steps: ReplayStep[]; state: TrayAutoFitState } {
  let state = EMPTY_AUTOFIT_STATE;
  let applied: number | null = null;
  const steps: ReplayStep[] = [];
  for (const measured of candidates) {
    const d = decideTrayHeight(
      input({ measuredHeight: measured, scaleFactor, lastAppliedPhysicalHeight: applied, ...overrides }),
      state,
    );
    state = d.state;
    if (d.commit) applied = Math.round(d.height * scaleFactor);
    steps.push({ measured, reason: d.reason, commit: d.commit, height: d.height });
  }
  return { steps, state };
}

function commitIndexes(steps: ReplayStep[]): number[] {
  return steps.flatMap((s, i) => (s.commit ? [i] : []));
}

describe("decideTrayHeight (#261 two-state cycle detection)", () => {
  it("commits the initial fit and clamps the measured height", () => {
    const d = decideTrayHeight(input({ measuredHeight: 539 }), EMPTY_AUTOFIT_STATE);
    expect(d).toMatchObject({ commit: true, reason: "initial", height: 539 });
    expect(decideTrayHeight(input({ measuredHeight: 50 }), EMPTY_AUTOFIT_STATE).height).toBe(MIN);
    expect(decideTrayHeight(input({ measuredHeight: 5000 }), EMPTY_AUTOFIT_STATE).height).toBe(MAX);
  });

  it("normal rule: every stable ONE-WAY small step commits (+5 physical each)", () => {
    // 480→484→488→492 logical @1.25 = 600→605→610→615 physical. NO blanket
    // deadband may absorb these: each is a real, stable change.
    const { steps } = replay(1.25, [480, 484, 488, 492]);
    expect(commitIndexes(steps)).toEqual([0, 1, 2, 3]);
    expect(steps.every((s) => s.reason === "initial" || s.reason === "commit")).toBe(true);
  });

  it("suppresses only exact same quantized frame (no neighborhood)", () => {
    const { steps } = replay(1.25, [480, 480, 481]);
    // 481→601.25≠600: even +1 physical px commits.
    expect(commitIndexes(steps)).toEqual([0, 2]);
    expect(steps[1]).toMatchObject({ commit: false, reason: "same-frame", height: 480 });
  });

  it("reporter pair: 379↔386 physical alternation detects once, converges to the LARGER member", () => {
    // 303.2↔308.8 logical @1.25 = exactly 379↔386 physical (reporter, issue #261).
    expect(Math.round(303.2 * 1.25)).toBe(379);
    expect(Math.round(308.8 * 1.25)).toBe(386);
    const { steps } = replay(1.25, [303.2, 308.8, 303.2, 303.2, 308.8, 303.2], { minHeight: 200 });
    expect(commitIndexes(steps)).toEqual([0, 1]);
    expect(steps[2]).toMatchObject({ commit: false, reason: "cycle-converge", height: 308.8 });
    for (const s of steps.slice(2)) {
      expect(s.commit).toBe(false);
      expect(s.reason).toMatch(/cycle/);
      // Retained height is ALWAYS the larger member — the window fully
      // contains the surface from either measured side (308.8 ≥ 303.2).
      expect(s.height).toBe(308.8);
    }
  });

  it("cycle detected below current position commits ONCE upward, then locks", () => {
    // A=500(625) → B=495(619) → A: committed is the lo member, so the detector
    // commits once to the larger (safe) member, then suppresses all flips.
    const { steps } = replay(1.25, [500, 495, 500, 495, 500, 495]);
    expect(commitIndexes(steps)).toEqual([0, 1, 2]);
    expect(steps[2]).toMatchObject({ commit: true, reason: "cycle-converge", height: 500 });
    expect(steps[3]).toMatchObject({ commit: false, reason: "cycle-suppress", height: 500 });
    expect(steps[4]).toMatchObject({ commit: false, reason: "cycle-suppress", height: 500 });
  });

  it("does NOT classify a lone A→B small change as oscillation", () => {
    const { steps, state } = replay(1.25, [303.2, 308.8, 308.8, 308.8], { minHeight: 200 });
    expect(commitIndexes(steps)).toEqual([0, 1]);
    expect(steps[2].reason).toBe("same-frame");
    expect(state.cycle).toBeNull();
  });

  it("does NOT cycle-detect pairs above the physical span cap", () => {
    expect(TRAY_CYCLE_SPAN_MAX_PHYSICAL_PX).toBe(8);
    // A=300→300, B=310→310 at sf=1: span 10 > 8 → genuine large flip, commits back.
    const { steps, state } = replay(1, [300, 310, 300], { minHeight: 200 });
    expect(commitIndexes(steps)).toEqual([0, 1, 2]);
    expect(state.cycle).toBeNull();
  });

  it("clears the lock and commits the moment a candidate lands OUTSIDE the learned pair", () => {
    const { steps, state } = replay(1.25, [303.2, 308.8, 303.2, 330], { minHeight: 200 });
    expect(commitIndexes(steps)).toEqual([0, 1, 3]);
    expect(steps[3]).toMatchObject({ commit: true, height: 330 });
    expect(state.cycle).toBeNull();
    // And values near—but not equal to—the old pair never stay suppressed.
    const { steps: near } = replay(1.25, [303.2, 308.8, 303.2, 306, 306], { minHeight: 200 });
    expect(commitIndexes(near)).toEqual([0, 1, 3]);
  });

  it("clears the lock on zoom change, DPI change, layout-class (min/max) change, and width change", () => {
    const base: number[] = [303.2, 308.8, 303.2];
    const locked = replay(1.25, base, { minHeight: 200 });
    expect(locked.state.cycle).not.toBeNull();

    // zoom 1 → 1.5 with an in-pair candidate: lock invalidated.
    const zoomed = decideTrayHeight(
      input({ measuredHeight: 308.8, scaleFactor: 1.25, zoom: 1.5, minHeight: 200 }),
      locked.state,
    );
    expect(zoomed.state.cycle).toBeNull();

    // DPI 1.25 → 1.0: physical frames incomparable → full reset, fresh fit.
    const dpi = decideTrayHeight(input({ measuredHeight: 308.8, scaleFactor: 1, minHeight: 200 }), locked.state);
    expect(dpi).toMatchObject({ commit: true, reason: "initial" });
    expect(dpi.state.cycle).toBeNull();

    // Layout class: min 200 → 420 with an outside-pair candidate → cleared + commit.
    const klass = decideTrayHeight(input({ measuredHeight: 500, scaleFactor: 1.25, minHeight: 420 }), locked.state);
    expect(klass.state.cycle).toBeNull();
    expect(klass.commit).toBe(true);

    // Width class change commits and clears.
    const wide = decideTrayHeight(input({ measuredHeight: 308.8, scaleFactor: 1.25, minHeight: 200, expectedWidth: 400 }), locked.state);
    expect(wide).toMatchObject({ commit: true, reason: "width" });
    expect(wide.state.cycle).toBeNull();
  });

  it("re-learns a pair only on fresh A→B→A evidence after a clear", () => {
    // Lock {379,386}, then a genuine stable move to 500 (lock cleared); a NEW
    // 500↔505 flip forms its own pair and converges again.
    const seq = replay(1.25, [303.2, 308.8, 303.2, 500, 505, 500, 500, 505], { minHeight: 200 });
    expect(commitIndexes(seq.steps)).toEqual([0, 1, 3, 4]);
    expect(seq.steps[5]).toMatchObject({ reason: "cycle-converge", height: 505 });
    expect(seq.steps[7]).toMatchObject({ commit: false, reason: "cycle-suppress", height: 505 });
  });

  it("reconciles committed state to the EXACT physical frame Win32 applied (readback)", () => {
    // History: min fit 420 (→525), then a real 480 commit (→600 target) whose
    // apply the OS snapped to applied=595. A 476 candidate (→595) equals the
    // on-screen frame: no setSize, and the committed frame is REPLACED by the
    // candidate's own {476,595} — the DOM constraint and future comparisons
    // now describe reality — while `prior` is left untouched (no manufactured
    // transition feeding the cycle detector).
    let state = recordAutoFitCommit(EMPTY_AUTOFIT_STATE, TRAY_WIDTH, 420, 1.25);
    state = recordAutoFitCommit(state, TRAY_WIDTH, 480, 1.25);
    expect(state.prior).toMatchObject({ height: 420, physical: 525 });

    const d = decideTrayHeight(
      input({ measuredHeight: 476, scaleFactor: 1.25, lastAppliedPhysicalHeight: 595 }), // 476→595
      state,
    );
    expect(d).toMatchObject({ commit: false, reason: "applied-frame", height: 476 });
    expect(d.state.committed).toMatchObject({
      width: TRAY_WIDTH,
      height: 476,
      physical: 595,
    });
    expect(d.state.prior).toMatchObject({ height: 420, physical: 525 });
    expect(d.state.cycle).toBeNull();

    // The very next identical candidate is now an exact same-frame no-op.
    const again = decideTrayHeight(
      input({ measuredHeight: 476, scaleFactor: 1.25, lastAppliedPhysicalHeight: 595 }),
      d.state,
    );
    expect(again).toMatchObject({ commit: false, reason: "same-frame", height: 476 });

    // …but only for EXACT equality: 596 physical is a real +1 change.
    const d2 = decideTrayHeight(
      input({ measuredHeight: 476.8, scaleFactor: 1.25, lastAppliedPhysicalHeight: 595, minHeight: 200 }),
      state,
    );
    expect(Math.round(476.8 * 1.25)).toBe(596);
    expect(d2.commit).toBe(true);
  });

  it("recordAutoFitCommit shifts history for hook-driven fits (min-fit seed)", () => {
    let s = recordAutoFitCommit(EMPTY_AUTOFIT_STATE, TRAY_WIDTH, 420, 1.25);
    expect(s.committed).toMatchObject({ width: TRAY_WIDTH, height: 420, physical: 525 });
    expect(s.prior).toBeNull();
    s = recordAutoFitCommit(s, TRAY_WIDTH, 200, 1.25);
    expect(s.committed).toMatchObject({ height: 200, physical: 250 });
    expect(s.prior).toMatchObject({ height: 420, physical: 525 });
  });

  it("zoom is measure-space: alternation around a zoom-scaled size still locks", () => {
    // content 500 × zoom 1.5 = 750 measured; pair 750↔754 (Δ5) still detects.
    const { steps } = replay(1, [750, 754, 750, 754], { zoom: 1.5 });
    expect(commitIndexes(steps)).toEqual([0, 1]);
    expect(steps[3]).toMatchObject({ commit: false, reason: "cycle-suppress", height: 754 });
  });
});
