/**
 * Reading progress and the folding header are scroll-driven animations in the stylesheet. Where
 * the browser has no scroll timelines, the bar and the fold follow the scroll from here: the bar
 * on its own transform, the fold through the same `--header-progress` the timeline would set, so
 * both browsers get the same header. The compact title the header folds into is measured once
 * per resize, and so are the header's natural heights where `calc-size()` is missing (Safari).
 * The measurements go on the header's inline style, which no component binds.
 */
export function installReadingHeader(signal: AbortSignal): void {
  const header = document.querySelector("body[data-kind='item'] .itemhead");
  if (!(header instanceof HTMLElement)) return;
  measureFold(header, signal);
  if (!CSS.supports('animation-timeline: scroll()')) driveFold(header, signal);
}

/** The scroll distance over which the header folds: the stylesheet's `animation-range: 0 10rem`. */
export const FOLD_RANGE_REM = 10;

/** The fold as a share of its range, for a scroll position. */
export function foldProgress(scrollY: number, remPx: number): number {
  const range = FOLD_RANGE_REM * remPx;
  return range > 0 ? Math.min(1, Math.max(0, scrollY / range)) : 0;
}

function driveFold(header: HTMLElement, signal: AbortSignal): void {
  const bar = header.querySelector('.itemhead-progress');
  let frame = 0;
  const paint = () => {
    frame = 0;
    const range = document.documentElement.scrollHeight - window.innerHeight;
    const progress = range > 0 ? Math.min(1, Math.max(0, window.scrollY / range)) : 0;
    if (bar instanceof HTMLElement) bar.style.transform = `scaleX(${progress})`;
    const rem = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
    header.style.setProperty('--header-progress', String(foldProgress(window.scrollY, rem)));
  };
  const schedule = () => {
    if (!frame) frame = requestAnimationFrame(paint);
  };
  window.addEventListener('scroll', schedule, { passive: true, signal });
  window.addEventListener('resize', schedule, { passive: true, signal });
  signal.addEventListener('abort', () => {
    if (frame) cancelAnimationFrame(frame);
    header.style.removeProperty('--header-progress');
  });
  paint();
}

function measureFold(header: HTMLElement, signal: AbortSignal): void {
  if (!('ResizeObserver' in window)) return;
  // The compact title the header folds into, set across the full width: its height is the one
  // the box closes to. Without calc-size() the natural heights are measured as well.
  const measured: Array<[Element | null, string]> = [[header.querySelector('.itemhead-title-compact'), '--compact-height']];
  if (!CSS.supports('height: calc-size(auto, size)')) {
    measured.push(
      [header.querySelector('.itemhead-title'), '--title-height'],
      [header.querySelector('.item-tags-inner'), '--tags-height'],
    );
  }
  // The untransformed border box: the title is scaled, and whole pixels would jolt the fold.
  const observer = new ResizeObserver((entries) => {
    for (const entry of entries) {
      const property = measured.find(([node]) => node === entry.target)?.[1];
      const height = entry.borderBoxSize?.[0]?.blockSize ?? entry.contentRect.height;
      if (property) header.style.setProperty(property, `${height}px`);
    }
  });
  for (const [node] of measured) if (node) observer.observe(node);
  signal.addEventListener('abort', () => observer.disconnect());
}
