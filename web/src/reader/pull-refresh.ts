/**
 * Pull to refresh in the installed app: a pure reducer decides what each touch does to the gesture, and a thin
 * binder feeds it the touch events. Rendering the indicator and reloading stay with the caller.
 */

export type PullPhase = "idle" | "tracking" | "pulling" | "armed" | "settling" | "refreshing";

export interface PullThresholds {
  /** Vertical travel that arms the refresh (px). */
  arm: number;
  /** Cap on the indicator distance (px). */
  max: number;
  /** Indicator distance held while the page reloads (px). */
  hold: number;
  /** Vertical travel below which the finger has not committed to a pull yet (px). */
  slop: number;
  /** Fraction of the finger travel the indicator follows. */
  ratio: number;
}

export const PULL_THRESHOLDS: PullThresholds = { arm: 84, max: 72, hold: 48, slop: 6, ratio: 0.55 };
/** How long the indicator takes to settle back before it is hidden (ms). */
export const PULL_SETTLE_MS = 190;

export type PullDecision =
  | { action: "ignore" }
  | { action: "settle" }
  | { action: "pull"; phase: "pulling" | "armed"; distance: number };

/** Whether a touch move can still develop into a refresh. */
export function pullTracking(phase: PullPhase): boolean {
  return phase === "tracking" || phase === "pulling" || phase === "armed";
}

/** What one touch move does to the gesture, from the finger's travel since the touch started. */
export function pullState(previous: PullPhase, deltaX: number, deltaY: number, thresholds: PullThresholds = PULL_THRESHOLDS): PullDecision {
  if (!pullTracking(previous)) return { action: "ignore" };
  if (deltaY <= 0 || Math.abs(deltaX) > deltaY) return { action: "settle" };
  if (deltaY < thresholds.slop) return { action: "ignore" };
  const distance = Math.min(thresholds.max, Math.round(deltaY * thresholds.ratio));
  return { action: "pull", phase: deltaY >= thresholds.arm ? "armed" : "pulling", distance };
}

/** What lifting the finger does: only an armed pull refreshes. */
export function pullRelease(previous: PullPhase): "refresh" | "settle" {
  return previous === "armed" ? "refresh" : "settle";
}

/** Whether settling has anything to undo: an idle indicator stays idle and a reload in progress keeps its state. */
export function pullSettles(previous: PullPhase): boolean {
  return previous !== "idle" && previous !== "refreshing";
}

export function pullLabel(phase: PullPhase): string {
  if (phase === "armed") return "Release to refresh";
  if (phase === "refreshing") return "Refreshing…";
  return "Pull to refresh";
}

export interface PullRefreshPorts {
  document: Document;
  window: Window;
  /** Whether a touch starting on `target` may begin a pull: installed, at the top, no dialog open, not editing. */
  canStart(target: EventTarget | null): boolean;
  /** Reflect the gesture in the page; `changed` is false when only the distance moved. */
  render(phase: PullPhase, distance: number, changed: boolean): void;
  /** Reload for new items; the indicator already shows the refreshing state. */
  refresh(): void;
}

/** Attach the touch listeners that drive the gesture. The caller decides once whether the page qualifies. */
export function wireTouchPullRefresh(ports: PullRefreshPorts, thresholds: PullThresholds = PULL_THRESHOLDS) {
  let phase: PullPhase = "idle";
  let startX = 0;
  let startY = 0;
  let resetTimer: ReturnType<typeof setTimeout> | undefined;

  function setPhase(next: PullPhase, distance: number) {
    const changed = phase !== next;
    phase = next;
    ports.render(next, distance, changed);
  }

  function settle() {
    if (!pullSettles(phase)) return;
    clearTimeout(resetTimer);
    setPhase("settling", 0);
    resetTimer = setTimeout(function () {
      setPhase("idle", 0);
    }, PULL_SETTLE_MS);
  }

  ports.document.addEventListener("touchstart", function (event) {
    if (phase === "refreshing" || event.touches.length !== 1) return;
    if (!ports.canStart(event.target)) return;
    clearTimeout(resetTimer);
    startX = event.touches[0].clientX;
    startY = event.touches[0].clientY;
    setPhase("tracking", 0);
  }, { passive: true });

  ports.document.addEventListener("touchmove", function (event) {
    if (!pullTracking(phase)) return;
    if (event.touches.length !== 1 || ports.window.scrollY > 0) {
      settle();
      return;
    }
    const decision = pullState(phase, event.touches[0].clientX - startX, event.touches[0].clientY - startY, thresholds);
    if (decision.action === "ignore") return;
    if (decision.action === "settle") {
      settle();
      return;
    }
    if (event.cancelable) event.preventDefault();
    setPhase(decision.phase, decision.distance);
  }, { passive: false });

  ports.document.addEventListener("touchend", function () {
    if (pullRelease(phase) === "refresh") {
      clearTimeout(resetTimer);
      setPhase("refreshing", thresholds.hold);
      ports.refresh();
    } else {
      settle();
    }
  }, { passive: true });
  ports.document.addEventListener("touchcancel", settle, { passive: true });
}
