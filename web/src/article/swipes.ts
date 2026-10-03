import type { Attachment } from 'svelte/attachments';

/**
 * Swiping the prose turns the page: left to the next older article, right to the previous newer
 * one. Reading gestures come first, so a swipe yields to a selection, a vertical scroll, a pinch,
 * a control, anything that scrolls sideways on its own (code, tables) and the browser's own edge
 * gestures. Nothing here cancels a touch: the stylesheet's `touch-action` on the prose leaves,
 * never on an ancestor, is what lets the page keep scrolling under a finger that was not swiping.
 */

/** The touch must start on prose; the stylesheet's `touch-action` covers the same elements. */
const PROSE = 'p, h1, h2, h3, h4, h5, h6, li, blockquote';
/** What keeps its own gestures, wherever it sits inside the prose. */
const OWN_GESTURES =
  'a, button, input, select, textarea, label, summary, audio, video, iframe, object, embed, canvas, pre, table, .table-scroll, [data-no-swipe], [contenteditable]';

/** Pixels from either side of the viewport the browser keeps for Back and Forward. */
const EDGE = 24;
/** How far sideways a swipe has to travel, and how little it may drift up or down. */
const DISTANCE = 72;
const DRIFT = 48;
/** A swipe is quick; a slow drag is something else. */
const TIME = 800;

/** Where a finished gesture goes: `1` next, `-1` previous, `0` nowhere. */
export function swipeOutcome(dx: number, dy: number, elapsed: number): -1 | 0 | 1 {
  if (elapsed > TIME || Math.abs(dx) < DISTANCE || Math.abs(dy) > DRIFT || Math.abs(dx) <= 2 * Math.abs(dy)) return 0;
  return dx < 0 ? 1 : -1;
}

/** Whether a gesture that has moved `dx, dy` is plainly a vertical scroll. */
export function vertical(dx: number, dy: number): boolean {
  return Math.abs(dy) > 12 && Math.abs(dy) > Math.abs(dx);
}

/** Whether anything between `target` and `region` scrolls sideways on its own. */
function scrollsSideways(target: Element, region: Element): boolean {
  for (let element: Element | null = target; element && element !== region; element = element.parentElement) {
    if (element.scrollWidth <= element.clientWidth + 1) continue;
    const overflow = getComputedStyle(element).overflowX;
    if (overflow === 'auto' || overflow === 'scroll') return true;
  }
  return false;
}

/** Whether a touch starting on `target` at `x` may become a swipe. */
export function swipeAllowed(target: EventTarget | null, region: HTMLElement, x: number, viewportWidth: number): boolean {
  if (!(target instanceof Element) || !region.contains(target)) return false;
  if (!target.closest(PROSE) || target.closest(OWN_GESTURES)) return false;
  if (x < EDGE || x > viewportWidth - EDGE) return false;
  if (scrollsSideways(target, region)) return false;
  const selection = target.ownerDocument.getSelection();
  return !selection || selection.isCollapsed;
}

/** Whether there is anything to swipe with. */
export function touchCapable(nav: Pick<Navigator, 'maxTouchPoints'> = navigator): boolean {
  return nav.maxTouchPoints > 0;
}

/**
 * The `<article>`'s own mark, where a finger can turn the page: the stylesheet scopes the prose
 * `touch-action` to it. Without touch the article stays exactly as the build rendered it.
 */
export const swipeable: Attachment<HTMLElement> = (node) => {
  if (!touchCapable()) return;
  node.setAttribute('data-swipe-navigation', '');
  return () => node.removeAttribute('data-swipe-navigation');
};

type Gesture = { x: number; y: number; scrollY: number; started: number };

export function installSwipes(region: HTMLElement, signal: AbortSignal, turn: (direction: 1 | -1) => void): void {
  let gesture: Gesture | null = null;
  const cancel = () => (gesture = null);
  region.addEventListener(
    'touchstart',
    (event) => {
      gesture = null;
      if (event.touches.length !== 1) return;
      const touch = event.touches[0];
      if (!swipeAllowed(event.target, region, touch.clientX, window.innerWidth)) return;
      gesture = { x: touch.clientX, y: touch.clientY, scrollY: window.scrollY, started: performance.now() };
    },
    { passive: true, signal },
  );
  region.addEventListener(
    'touchmove',
    (event) => {
      if (!gesture) return;
      const touch = event.touches[0];
      // A second finger is a pinch; a page that moved was being scrolled, not turned.
      if (event.touches.length !== 1 || !touch || window.scrollY !== gesture.scrollY) return cancel();
      if (vertical(touch.clientX - gesture.x, touch.clientY - gesture.y)) cancel();
    },
    { passive: true, signal },
  );
  region.addEventListener(
    'touchend',
    (event) => {
      const started = gesture;
      gesture = null;
      const touch = event.changedTouches[0];
      if (!started || !touch || event.touches.length || window.scrollY !== started.scrollY) return;
      // A finger that ended up selecting text was reading, not turning.
      const selection = getSelection();
      if (selection && !selection.isCollapsed) return;
      const outcome = swipeOutcome(touch.clientX - started.x, touch.clientY - started.y, performance.now() - started.started);
      if (outcome) turn(outcome);
    },
    { passive: true, signal },
  );
  region.addEventListener('touchcancel', cancel, { passive: true, signal });
}
