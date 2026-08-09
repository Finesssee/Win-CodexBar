/**
 * Pure auto-fit sizing decision for the anchored tray flyout (#261).
 *
 * Problem: on fractional-DPI setups (reporter: Win11 25H2 at 125% scale,
 * AppliedDPI 120) the transparent borderless flyout alternated natively
 * between 379 and 386 physical px (≈5.6 logical px) at 2–4 Hz: a bounded
 * two-position measure↔setSize↔layout feedback loop. The exact OS driver
 * is unconfirmed (a same-layout Chromium sandbox at deviceScaleFactor 1.25
 * converges), so this targets the demonstrated FEEDBACK CLASS, not a guess
 * at the OS cause: explicit two-state cycle detection on physical targets.
 *
 * Policy (narrow):
 * - NORMAL RULE: any candidate whose quantized PHYSICAL height differs
 *   from the committed frame commits — including a stable one-way +5 px.
 *   Suppression happens ONLY for exact same-frame equality (a genuine
 *   no-op) — there is deliberately NO neighborhood deadband, so small real
 *   deltas are never absorbed.
 * - CYCLE DETECTION: history of the last two committed physical targets.
 *   A candidate returning to the previous-previous frame (A→B→A) with pair
 *   span ≤ TRAY_CYCLE_SPAN_MAX_PHYSICAL_PX (reporter observed 7; cap 8)
 *   is an oscillation pair. A lone A→B small change is NOT classified as
 *   oscillation — it commits normally.
 * - Once a pair is learned, the LARGER member is retained (window fully
 *   contains the surface, no clipping) and candidates equal to EITHER pair
 *   member are suppressed until something outside the pair arrives.
 * - The lock clears on: a candidate outside the pair (real change), a
 *   width change, a min/max (layout-class) change, a zoom change, or a DPI
 *   (scaleFactor) change — so unrelated nearby values are never suppressed
 *   indefinitely.
 *
 * No timers, no debounce, no observer gating: measurement always runs;
 * only setSize/re-anchor commits are decided here.
 */

/** Max span (physical px) of a recognized two-state feedback pair. Reporter
 *  evidence: 7 physical px at 125% scale; capped at 8. */
export const TRAY_CYCLE_SPAN_MAX_PHYSICAL_PX = 8;

export interface TrayFrame {
  /** Logical-px height committed/applied to the window. */
  height: number;
  /** Quantized physical height: round(height * scaleFactor) at commit. */
  physical: number;
}

interface CycleLock {
  lo: TrayFrame;
  hi: TrayFrame;
  scaleFactor: number;
  zoom: number;
  minHeight: number;
  maxHeight: number;
}

/** Full decision state; owned by the hook in a ref, threaded pure. */
export interface TrayAutoFitState {
  /** Last committed frame (null before the first fit). */
  committed: (TrayFrame & { width: number }) | null;
  /** The frame committed BEFORE `committed` (the A in A→B→A). */
  prior: TrayFrame | null;
  /** Learned oscillation pair after two-state detection. */
  cycle: CycleLock | null;
  /** Scale factor all physical values above were computed with. */
  scaleFactor: number | null;
}

export const EMPTY_AUTOFIT_STATE: TrayAutoFitState = {
  committed: null,
  prior: null,
  cycle: null,
  scaleFactor: null,
};

export interface TraySizingInput {
  /** Measured content height, logical px, post zoom-scale, pre clamp. */
  measuredHeight: number;
  /** Logical content width the flyout always carries (TRAY_WIDTH). */
  expectedWidth: number;
  minHeight: number;
  maxHeight: number;
  /** Win32 DPI ratio (window.devicePixelRatio), e.g. 1.25 at 125%. */
  scaleFactor: number;
  /** Active tray zoom (CSS zoom factor already applied to the measure). */
  zoom: number;
  /** Physical height Win32 ACTUALLY applied after our last resize
   *  (`innerSize()` readback); used only for exact-frame equality. */
  lastAppliedPhysicalHeight: number | null;
}

export type TrayDecisionReason =
  | "initial"
  | "width"
  | "commit"
  | "same-frame"
  | "applied-frame"
  | "cycle-converge"
  | "cycle-suppress";

export interface TrayAutoFitDecision {
  /** Logical height the DOM constraint AND any window apply must use.
   *  On suppression this is ALWAYS the retained committed height, never
   *  the freshly measured candidate. */
  height: number;
  commit: boolean;
  reason: TrayDecisionReason;
  state: TrayAutoFitState;
}

/** Record a commit the decision function did not order (initial min fit). */
export function recordAutoFitCommit(
  state: TrayAutoFitState,
  width: number,
  height: number,
  scaleFactor: number,
): TrayAutoFitState {
  const frame = { height, physical: Math.round(height * scaleFactor) };
  return {
    committed: { ...frame, width },
    prior: state.committed
      ? { height: state.committed.height, physical: state.committed.physical }
      : state.prior,
    cycle: state.cycle,
    scaleFactor,
  };
}

export function decideTrayHeight(
  input: TraySizingInput,
  prevState: TrayAutoFitState,
): TrayAutoFitDecision {
  const sf =
    Number.isFinite(input.scaleFactor) && input.scaleFactor > 0
      ? input.scaleFactor
      : 1;
  const zoom = Number.isFinite(input.zoom) && input.zoom > 0 ? input.zoom : 1;
  const height = Math.min(
    Math.max(input.measuredHeight, input.minHeight),
    input.maxHeight,
  );
  const candidatePhysical = Math.round(height * sf);

  // DPI changed: physical frames are incomparable — reset all history.
  let state =
    prevState.scaleFactor !== null && prevState.scaleFactor !== sf
      ? { ...EMPTY_AUTOFIT_STATE, scaleFactor: sf }
      : { ...prevState, scaleFactor: sf };

  // Validate or drop the learned cycle before anything consults it.
  if (state.cycle) {
    const lock = state.cycle;
    const contextUnchanged =
      lock.scaleFactor === sf &&
      lock.zoom === zoom &&
      lock.minHeight === input.minHeight &&
      lock.maxHeight === input.maxHeight &&
      state.committed !== null &&
      state.committed.width === input.expectedWidth;
    const inPair =
      candidatePhysical === lock.lo.physical ||
      candidatePhysical === lock.hi.physical;
    if (!contextUnchanged) {
      state = { ...state, cycle: null };
    } else if (inPair) {
      // Still flipping inside the learned pair: retain the LARGER member —
      // the window fully contains the surface in either measure state.
      return { height: lock.hi.height, commit: false, reason: "cycle-suppress", state };
    } else {
      // Real change outside the pair: unlock and fall through to the
      // normal rule with history intact.
      state = { ...state, cycle: null };
    }
  }

  if (state.committed === null) {
    return {
      height,
      commit: true,
      reason: "initial",
      state: {
        committed: { height, physical: candidatePhysical, width: input.expectedWidth },
        prior: null,
        cycle: null,
        scaleFactor: sf,
      },
    };
  }

  const committed = state.committed;
  const pushCommit = (): TrayAutoFitState => ({
    committed: { height, physical: candidatePhysical, width: input.expectedWidth },
    prior: { height: committed.height, physical: committed.physical },
    cycle: null,
    scaleFactor: sf,
  });

  if (committed.width !== input.expectedWidth) {
    return { height, commit: true, reason: "width", state: pushCommit() };
  }

  // Exact same quantized frame as committed: a genuine no-op (for integer
  // logical measures at sf ≥ 1, physical equality ⇔ logical equality).
  if (candidatePhysical === committed.physical) {
    return { height: committed.height, commit: false, reason: "same-frame", state };
  }

  // A→B→A bounded two-state detection: candidate returns to the frame we
  // committed before `committed`, at a span within the observed feedback
  // amplitude. A lone A→B never reaches this branch.
  const prior = state.prior;
  if (
    prior !== null &&
    candidatePhysical === prior.physical &&
    Math.abs(candidatePhysical - committed.physical) <= TRAY_CYCLE_SPAN_MAX_PHYSICAL_PX
  ) {
    const loFrame = prior.physical <= committed.physical ? prior : committed;
    const hiFrame = prior.physical <= committed.physical ? committed : prior;
    const cycle: CycleLock = {
      lo: loFrame,
      hi: hiFrame,
      scaleFactor: sf,
      zoom,
      minHeight: input.minHeight,
      maxHeight: input.maxHeight,
    };
    if (committed.physical === hiFrame.physical) {
      // Already at the larger member: hold it, apply nothing.
      return {
        height: hiFrame.height,
        commit: false,
        reason: "cycle-converge",
        state: { ...state, cycle },
      };
    }
    // Retain the LARGER member with one convergence commit, then lock.
    return {
      height,
      commit: true,
      reason: "cycle-converge",
      state: {
        committed: { height, physical: candidatePhysical, width: input.expectedWidth },
        prior: { height: committed.height, physical: committed.physical },
        cycle,
        scaleFactor: sf,
      },
    };
  }

  // Idempotence anchored in reality: the candidate's physical frame IS what
  // Win32 currently shows (snap/clamp made the applied size differ from the
  // recorded target). Re-applying would be a rect no-op, so suppress the
  // setSize — and reconcile state to reality: the committed frame becomes
  // the candidate's (logical height + physical), so the DOM constraint and
  // any later comparisons use what is actually on screen. `prior` is left
  // untouched: this is not a committed transition, so it must not feed the
  // cycle detector's A→B→A evidence.
  if (
    input.lastAppliedPhysicalHeight !== null &&
    candidatePhysical === input.lastAppliedPhysicalHeight
  ) {
    return {
      height,
      commit: false,
      reason: "applied-frame",
      state: {
        ...state,
        committed: { height, physical: candidatePhysical, width: input.expectedWidth },
      },
    };
  }

  return { height, commit: true, reason: "commit", state: pushCommit() };
}
