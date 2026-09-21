import { afterEach, describe, expect, it, vi } from "vitest";
import { mountArticleSwipe } from "./article-swipe";

function harness() {
  const listeners = new Map<string, Set<EventListener>>();
  const view = { innerWidth: 390, scrollX: 0, scrollY: 100, getSelection: () => ({ isCollapsed: true }), getComputedStyle: () => ({ overflowX: "visible" }) };
  const document = {
    defaultView: view,
    addEventListener: vi.fn((name: string, fn: EventListener) => {
      if (!listeners.has(name)) listeners.set(name, new Set());
      listeners.get(name)!.add(fn);
    }),
    removeEventListener: vi.fn((name: string, fn: EventListener) => {
      listeners.get(name)?.delete(fn);
      if (!listeners.get(name)?.size) listeners.delete(name);
    }),
  };
  const root = { ownerDocument: document, contains: (target: unknown) => target === targetElement };
  const targetElement = { nodeType: 1, closest: vi.fn(() => null), parentElement: root, scrollWidth: 200, clientWidth: 200 };
  const previous = vi.fn((): string | null => "/previous/");
  const next = vi.fn((): string | null => "/next/");
  const navigate = vi.fn();
  const isBusy = vi.fn(() => false);
  const dispose = mountArticleSwipe(root as unknown as HTMLElement, { previous, next, navigate, isBusy });
  const pointer = (name: string, x: number, y = 100, extra: Record<string, unknown> = {}) => {
    const event = { target: targetElement, pointerId: 1, pointerType: "touch", isPrimary: true, button: 0, clientX: x, clientY: y, timeStamp: 100, cancelable: true, preventDefault: vi.fn(), stopImmediatePropagation: vi.fn(), ...extra };
    for (const listener of [...(listeners.get(name) ?? [])]) listener(event as unknown as Event);
    return event;
  };
  return { listeners, view, root, targetElement, previous, next, navigate, isBusy, dispose, pointer };
}

afterEach(() => vi.useRealTimers());

describe("article swipes", () => {
  it.each([[-100, "/next/"], [100, "/previous/"]])("navigates once after a deliberate horizontal swipe %s", (distance, url) => {
    const h = harness();
    h.pointer("pointerdown", 190);
    h.pointer("pointermove", 190 + distance / 2);
    h.pointer("pointerup", 190 + distance);
    h.pointer("pointerup", 190 + distance);
    expect(h.navigate).toHaveBeenCalledExactlyOnceWith(url);
    h.dispose();
  });

  it("snapshots adjacent URLs at the start and does not navigate while busy", () => {
    const h = harness();
    h.pointer("pointerdown", 190);
    h.next.mockReturnValue("/changed/");
    h.pointer("pointermove", 110);
    h.pointer("pointerup", 100);
    expect(h.navigate).toHaveBeenCalledWith("/next/");
    h.isBusy.mockReturnValue(true);
    h.pointer("pointerdown", 190);
    h.pointer("pointerup", 90);
    expect(h.navigate).toHaveBeenCalledTimes(1);
    h.dispose();
  });

  it.each([[0, 0], [25, 0], [70, 60], [15, 100]])("ignores taps, short drags, diagonals and vertical scrolling (%s,%s)", (x, y) => {
    const h = harness();
    h.pointer("pointerdown", 190);
    const move = h.pointer("pointermove", 190 + x, 100 + y);
    h.pointer("pointerup", 190 + x, 100 + y);
    expect(h.navigate).not.toHaveBeenCalled();
    if (y > 0 || x === 0) expect(move.preventDefault).not.toHaveBeenCalled();
    h.dispose();
  });

  it("cannot turn a vertical gesture or a direction reversal into navigation", () => {
    for (const first of [[190, 120], [220, 100]]) {
      const h = harness();
      h.pointer("pointerdown", 190);
      h.pointer("pointermove", first[0], first[1]);
      h.pointer("pointermove", 90);
      h.pointer("pointerup", 80);
      expect(h.navigate).not.toHaveBeenCalled();
      h.dispose();
    }
  });

  it.each(["mouse", "pen"])("ignores %s input", pointerType => {
    const h = harness();
    h.pointer("pointerdown", 190, 100, { pointerType });
    h.pointer("pointerup", 90, 100, { pointerType });
    expect(h.navigate).not.toHaveBeenCalled();
    h.dispose();
  });

  it.each([10, 380])("reserves viewport edge %s for browser gestures", x => {
    const h = harness();
    h.pointer("pointerdown", x);
    h.pointer("pointerup", 190);
    expect(h.navigate).not.toHaveBeenCalled();
    h.dispose();
  });

  it("cancels multi-touch until every finger is released, including a finger outside the article", () => {
    const h = harness();
    h.pointer("pointerdown", 190);
    h.pointer("pointermove", 150);
    h.pointer("pointerdown", 210, 100, { pointerId: 2, isPrimary: false, target: null });
    h.pointer("pointerup", 80);
    h.pointer("pointerup", 80, 100, { pointerId: 2 });
    expect(h.navigate).not.toHaveBeenCalled();
    h.pointer("pointerdown", 190);
    h.pointer("pointermove", 100);
    h.pointer("pointerup", 80);
    expect(h.navigate).toHaveBeenCalledOnce();
    h.dispose();
  });

  it("excludes interactive descendants and horizontal scroll containers", () => {
    const h = harness();
    h.targetElement.closest.mockReturnValue({} as never);
    h.pointer("pointerdown", 190);
    h.pointer("pointerup", 90);
    expect(h.targetElement.closest).toHaveBeenCalledWith(expect.stringContaining("a[href]"));
    expect(h.navigate).not.toHaveBeenCalled();
    h.targetElement.closest.mockReturnValue(null);
    h.targetElement.scrollWidth = 400;
    h.view.getComputedStyle = () => ({ overflowX: "auto" });
    h.pointer("pointerdown", 190);
    h.pointer("pointerup", 90);
    expect(h.navigate).not.toHaveBeenCalled();
    h.dispose();
  });

  it.each(["selection", "scroll", "busy", "cancel", "long-press", "dispose"])("abandons navigation after %s", reason => {
    const h = harness();
    h.pointer("pointerdown", 190);
    h.pointer("pointermove", 150);
    if (reason === "selection") h.view.getSelection = () => ({ isCollapsed: false });
    if (reason === "scroll") h.view.scrollY += 10;
    if (reason === "busy") h.isBusy.mockReturnValue(true);
    if (reason === "cancel") h.pointer("pointercancel", 150);
    if (reason === "dispose") h.dispose();
    h.pointer("pointerup", 90, 100, { timeStamp: reason === "long-press" ? 2000 : 200 });
    expect(h.navigate).not.toHaveBeenCalled();
    h.dispose();
  });

  it("does not claim a swipe toward a missing adjacent article", () => {
    const h = harness();
    h.next.mockReturnValue(null);
    h.pointer("pointerdown", 190);
    const move = h.pointer("pointermove", 100);
    h.pointer("pointerup", 90);
    expect(move.preventDefault).not.toHaveBeenCalled();
    expect(h.navigate).not.toHaveBeenCalled();
    h.dispose();
  });

  it("suppresses the matching trailing click even when navigation disposes the page", () => {
    vi.useFakeTimers();
    const h = harness();
    h.navigate.mockImplementation(h.dispose);
    h.pointer("pointerdown", 190);
    h.pointer("pointermove", 100);
    h.pointer("pointerup", 90);
    const click = h.pointer("click", 90, 100, { detail: 1 });
    expect(click.preventDefault).toHaveBeenCalledOnce();
    expect(click.stopImmediatePropagation).toHaveBeenCalledOnce();
    vi.advanceTimersByTime(600);
    expect(h.listeners.size).toBe(0);
  });

  it("does not suppress keyboard clicks or later intentional input", () => {
    vi.useFakeTimers();
    const h = harness();
    h.pointer("pointerdown", 190);
    h.pointer("pointermove", 100);
    h.pointer("pointerup", 90);
    expect(h.pointer("click", 90, 100, { detail: 0 }).preventDefault).not.toHaveBeenCalled();
    h.pointer("pointerdown", 90);
    expect(h.pointer("click", 90, 100, { detail: 1 }).preventDefault).not.toHaveBeenCalled();
    h.dispose();
  });
});
