import type { ArticleHeader } from "../contracts";
import { $, setStyle } from "./dom";

/** Scrolling this far folds the article header down to its title. */
export const HEADER_FOLD_PX = 160;

function clamp(value: number) {
  return Math.max(0, Math.min(1, value));
}

/** How far the header has folded at scroll offset `y`. */
export function headerFold(y: number): { folded: boolean; progress: number } {
  return { folded: y >= HEADER_FOLD_PX, progress: clamp(y / HEADER_FOLD_PX) };
}

/** How much of the article has been read at scroll offset `y`; a page that does not scroll reads as nothing. */
export function readingProgress(y: number, scrollRange: number): number {
  return scrollRange > 0 ? clamp(y / scrollRange) : 0;
}

export interface ArticleHeaderPorts {
  document: Document;
  /** The real window: `ResizeObserver` and the frame scheduler come from it. */
  window: Window & typeof globalThis;
  kind(): string;
}

/**
 * The folding article header and its reading-progress bar. `wire()` adopts the current page's header
 * (or none, off article pages); `dispose()` releases it before the page is replaced. Scroll and resize
 * listeners are attached once, for the life of the app.
 */
export function createArticleHeader(ports: ArticleHeaderPorts) {
  const { document, window } = ports;
  let observer: ResizeObserver | undefined;
  let frame: number | null = null;
  let state: ArticleHeader | null = null;
  let needsMeasure = true;

  function update() {
    frame = null;
    const current = state;
    if (!current) return;
    const y = window.scrollY;
    const fold = headerFold(y);
    // Read geometry before changing the folding styles, so scrolling never forces
    // a synchronous layout after an earlier write in this frame.
    let measurements;
    if (needsMeasure) {
      const style = window.getComputedStyle(current.header);
      const top = current.topBar ? Math.max(0, current.topBar.getBoundingClientRect().bottom) : 0;
      measurements = {
        tags: current.labels ? current.labels.getBoundingClientRect().height : null,
        // offsetHeight rounds to whole pixels; the title's transform must not affect this height.
        titleHeight: current.title ? parseFloat(window.getComputedStyle(current.title).height) : null,
        offset: (style.position === "sticky" ? current.header.getBoundingClientRect().height + (parseFloat(style.top) || 0) : top)
          + (current.fade ? current.fade.getBoundingClientRect().height : 16),
        range: document.documentElement.scrollHeight - window.innerHeight
      };
      current.scrollRange = measurements.range;
      needsMeasure = false;
    }
    setStyle(current.header, "--header-progress", fold.progress);
    if (current.tags && current.folded !== fold.folded) {
      current.tags.inert = fold.folded;
      current.tags.style.visibility = fold.folded ? "hidden" : "visible";
    }
    current.folded = fold.folded;
    if (measurements) {
      if (measurements.tags !== null) setStyle(current.header, "--item-tags-height", measurements.tags + "px");
      if (measurements.titleHeight !== null) setStyle(current.header, "--header-title-height", measurements.titleHeight + "px");
      setStyle(document.documentElement, "--article-header-offset", measurements.offset + "px");
    }
    if (current.progress) setStyle(current.progress, "transform", `scaleX(${readingProgress(y, current.scrollRange)})`);
  }

  function schedule() {
    if (state && !frame) frame = window.requestAnimationFrame(update);
  }

  function measure() {
    needsMeasure = true;
    schedule();
  }

  function wire() {
    if (observer) observer.disconnect();
    document.documentElement.style.removeProperty("--article-header-offset");
    const header = ports.kind() === "item" && $(".itemhead", document);
    state = header ? {
      header: header, title: $(".itemhead-title", header), tags: $(".item-tags", header), labels: $(".item-tags-inner", header),
      progress: $(".itemhead-progress", header), fade: $(".itemhead-fade", header), topBar: $(".top", document), scrollRange: 0
    } : null;
    needsMeasure = true;
    schedule();
    if (!header) return;
    if (window.ResizeObserver) {
      observer = new window.ResizeObserver(measure);
      observer.observe(header);
      const main = $("main", document);
      if (main) observer.observe(main);
      if (state?.labels) observer.observe(state.labels);
      if (state?.title) observer.observe(state.title);
    }
  }

  function dispose() {
    observer?.disconnect();
    if (frame !== null) window.cancelAnimationFrame(frame);
    observer = undefined;
    frame = null;
    state = null;
  }

  window.addEventListener("scroll", schedule, { passive: true });
  window.addEventListener("resize", measure, { passive: true });
  return { wire, schedule, measure, dispose };
}
