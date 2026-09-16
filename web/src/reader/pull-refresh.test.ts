import { describe, expect, it, vi } from "vitest";
import { PULL_SETTLE_MS, PULL_THRESHOLDS, pullLabel, pullRelease, pullSettles, pullState, pullTracking, wireTouchPullRefresh } from "./pull-refresh";

describe("pull state reducer", () => {
  it("ignores moves outside a tracked gesture", () => {
    expect(pullState("idle", 0, 120)).toEqual({ action: "ignore" });
    expect(pullState("settling", 0, 120)).toEqual({ action: "ignore" });
    expect(pullState("refreshing", 0, 120)).toEqual({ action: "ignore" });
    expect(pullTracking("tracking") && pullTracking("pulling") && pullTracking("armed")).toBe(true);
  });

  it("settles when the finger moves up or more sideways than down", () => {
    expect(pullState("tracking", 0, 0)).toEqual({ action: "settle" });
    expect(pullState("pulling", 0, -4)).toEqual({ action: "settle" });
    expect(pullState("armed", 40, 30)).toEqual({ action: "settle" });
    expect(pullState("armed", -40, 30)).toEqual({ action: "settle" });
  });

  it("waits for the slop distance before it becomes a pull", () => {
    expect(pullState("tracking", 0, 5)).toEqual({ action: "ignore" });
    expect(pullState("tracking", 0, 6)).toEqual({ action: "pull", phase: "pulling", distance: 3 });
  });

  it("follows the finger at the configured ratio, caps the indicator and arms at the threshold", () => {
    expect(pullState("tracking", 2, 40)).toEqual({ action: "pull", phase: "pulling", distance: 22 });
    expect(pullState("pulling", 0, 83)).toEqual({ action: "pull", phase: "pulling", distance: 46 });
    expect(pullState("pulling", 0, 84)).toEqual({ action: "pull", phase: "armed", distance: 46 });
    expect(pullState("armed", 0, 400)).toEqual({ action: "pull", phase: "armed", distance: PULL_THRESHOLDS.max });
    expect(pullState("tracking", 0, 20, { arm: 10, max: 5, hold: 1, slop: 1, ratio: 1 })).toEqual({ action: "pull", phase: "armed", distance: 5 });
  });

  it("refreshes only from an armed release and settles only from a live gesture", () => {
    expect(pullRelease("armed")).toBe("refresh");
    expect(pullRelease("pulling")).toBe("settle");
    expect(pullRelease("tracking")).toBe("settle");
    expect(pullSettles("idle")).toBe(false);
    expect(pullSettles("refreshing")).toBe(false);
    expect(pullSettles("pulling")).toBe(true);
    expect(pullSettles("settling")).toBe(true);
  });

  it("labels the indicator by phase", () => {
    expect(pullLabel("armed")).toBe("Release to refresh");
    expect(pullLabel("refreshing")).toBe("Refreshing…");
    expect(pullLabel("pulling")).toBe("Pull to refresh");
    expect(pullLabel("tracking")).toBe("Pull to refresh");
  });
});

describe("touch binder", () => {
  function harness(canStart = true) {
    const listeners = new Map<string, (event: unknown) => void>();
    const document = { addEventListener: vi.fn((name: string, handler: (event: unknown) => void) => { listeners.set(name, handler); }) };
    const window = { scrollY: 0 };
    const render = vi.fn();
    const refresh = vi.fn();
    wireTouchPullRefresh({ document: document as unknown as Document, window: window as unknown as Window, canStart: () => canStart, render, refresh });
    const touch = (name: string, points: Array<[number, number]>, cancelable = true) => {
      const event = { touches: points.map(([clientX, clientY]) => ({ clientX, clientY })), target: null, cancelable, preventDefault: vi.fn() };
      listeners.get(name)!(event);
      return event;
    };
    return { listeners, window, render, refresh, touch };
  }

  it("binds the four touch events with the scroll-preserving passive flags", () => {
    const { listeners } = harness();
    expect([...listeners.keys()]).toEqual(["touchstart", "touchmove", "touchend", "touchcancel"]);
  });

  it("tracks, pulls, arms and refreshes with the held distance", () => {
    const { render, refresh, touch } = harness();
    touch("touchstart", [[10, 100]]);
    expect(render).toHaveBeenLastCalledWith("tracking", 0, true);
    const small = touch("touchmove", [[10, 104]]);
    expect(small.preventDefault).not.toHaveBeenCalled();
    const pull = touch("touchmove", [[12, 140]]);
    expect(pull.preventDefault).toHaveBeenCalled();
    expect(render).toHaveBeenLastCalledWith("pulling", 22, true);
    touch("touchmove", [[12, 150]]);
    expect(render).toHaveBeenLastCalledWith("pulling", 28, false);
    touch("touchmove", [[12, 200]]);
    expect(render).toHaveBeenLastCalledWith("armed", 55, true);
    touch("touchend", []);
    expect(render).toHaveBeenLastCalledWith("refreshing", PULL_THRESHOLDS.hold, true);
    expect(refresh).toHaveBeenCalledTimes(1);
    touch("touchstart", [[10, 100]]);
    expect(render).toHaveBeenLastCalledWith("refreshing", PULL_THRESHOLDS.hold, true);
  });

  it("does not start when the page disqualifies the touch or several fingers touch", () => {
    const blocked = harness(false);
    blocked.touch("touchstart", [[10, 100]]);
    expect(blocked.render).not.toHaveBeenCalled();
    const two = harness();
    two.touch("touchstart", [[10, 100], [40, 100]]);
    expect(two.render).not.toHaveBeenCalled();
    two.touch("touchmove", [[10, 200]]);
    expect(two.render).not.toHaveBeenCalled();
  });

  it("settles back to idle after the settle delay when the pull is abandoned", () => {
    vi.useFakeTimers();
    try {
      const { render, refresh, touch, window } = harness();
      touch("touchstart", [[10, 100]]);
      touch("touchmove", [[10, 140]]);
      touch("touchend", []);
      expect(refresh).not.toHaveBeenCalled();
      expect(render).toHaveBeenLastCalledWith("settling", 0, true);
      vi.advanceTimersByTime(PULL_SETTLE_MS - 1);
      expect(render).toHaveBeenLastCalledWith("settling", 0, true);
      vi.advanceTimersByTime(1);
      expect(render).toHaveBeenLastCalledWith("idle", 0, true);
      touch("touchstart", [[10, 100]]);
      window.scrollY = 12;
      touch("touchmove", [[10, 140]]);
      expect(render).toHaveBeenLastCalledWith("settling", 0, true);
      touch("touchcancel", []);
      expect(render).toHaveBeenLastCalledWith("settling", 0, false);
    } finally {
      vi.useRealTimers();
    }
  });
});
