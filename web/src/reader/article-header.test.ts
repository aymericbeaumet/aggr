import { describe, expect, it, vi } from "vitest";
import { HEADER_FOLD_PX, createArticleHeader, headerFold, readingProgress } from "./article-header";

describe("article header geometry", () => {
  it("folds at the threshold and reports the fraction folded", () => {
    expect(headerFold(0)).toEqual({ folded: false, progress: 0 });
    expect(headerFold(80)).toEqual({ folded: false, progress: 0.5 });
    expect(headerFold(HEADER_FOLD_PX)).toEqual({ folded: true, progress: 1 });
    expect(headerFold(5000)).toEqual({ folded: true, progress: 1 });
    expect(headerFold(-20)).toEqual({ folded: false, progress: 0 });
  });

  it("measures reading progress within the scroll range and reads a fixed page as unread", () => {
    expect(readingProgress(0, 1000)).toBe(0);
    expect(readingProgress(250, 1000)).toBe(0.25);
    expect(readingProgress(2000, 1000)).toBe(1);
    expect(readingProgress(100, 0)).toBe(0);
    expect(readingProgress(100, -5)).toBe(0);
  });
});

describe("article header lifecycle", () => {
  function harness(kind: string, header: object | null) {
    const listeners = new Map<string, () => void>();
    const documentElement = { style: { removeProperty: vi.fn() }, scrollHeight: 2000 };
    const document = { documentElement, querySelector: vi.fn((selector: string) => selector === ".itemhead" ? header : null) };
    const window = {
      scrollY: 0, innerHeight: 800,
      addEventListener: vi.fn((name: string, handler: () => void) => { listeners.set(name, handler); }),
      requestAnimationFrame: vi.fn(() => 7), cancelAnimationFrame: vi.fn(),
      ResizeObserver: undefined
    };
    const controller = createArticleHeader({ document: document as unknown as Document, window: window as unknown as Window & typeof globalThis, kind: () => kind });
    return { controller, document, window, listeners };
  }

  it("listens for scroll and resize once and only schedules frames for an article header", () => {
    const feed = harness("river", { querySelector: () => null });
    expect([...feed.listeners.keys()]).toEqual(["scroll", "resize"]);
    feed.controller.wire();
    expect(feed.document.documentElement.style.removeProperty).toHaveBeenCalledWith("--article-header-offset");
    feed.listeners.get("scroll")!();
    expect(feed.window.requestAnimationFrame).not.toHaveBeenCalled();

    const article = harness("item", { querySelector: () => null });
    article.controller.wire();
    expect(article.window.requestAnimationFrame).toHaveBeenCalledTimes(1);
    article.listeners.get("scroll")!();
    expect(article.window.requestAnimationFrame).toHaveBeenCalledTimes(1);
  });

  it("releases the pending frame and forgets the header on disposal", () => {
    const article = harness("item", { querySelector: () => null });
    article.controller.wire();
    article.controller.dispose();
    expect(article.window.cancelAnimationFrame).toHaveBeenCalledWith(7);
    article.controller.schedule();
    expect(article.window.requestAnimationFrame).toHaveBeenCalledTimes(1);
  });
});
