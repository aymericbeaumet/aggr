/**
 * Small platform stand-ins the stylesheet relies on: they set classes and attributes on the
 * document and its static chrome, never on nodes a component owns.
 */

import { tabAt } from '../navigation/tabs';

/** While Shift is held, links reveal where they lead. Styled entirely by the stylesheet. */
export function installShiftHover(): void {
  const held = (on: boolean) => document.documentElement.classList.toggle('is-shift-held', on);
  const options = { capture: true, passive: true };
  document.addEventListener('keydown', (event) => held(event.shiftKey), options);
  document.addEventListener('keyup', (event) => held(event.shiftKey), options);
  document.addEventListener('pointerover', (event) => held(event.shiftKey), options);
  document.addEventListener('visibilitychange', () => document.hidden && held(false), options);
  window.addEventListener('blur', () => held(false), options);
  window.addEventListener('pagehide', () => held(false), options);
}

/**
 * Touch has no hover to fall back on, and `:active` waits to find out whether a touch was
 * really a scroll. A tab says it was hit the moment it is touched; the stylesheet does the rest.
 */
export function installTapFeedback(): void {
  const clear = () => {
    for (const pressed of document.querySelectorAll('.mobile-tabs [data-pressed]')) pressed.removeAttribute('data-pressed');
  };
  document.addEventListener(
    'pointerdown',
    (event) => {
      const target = event.target;
      if (!(target instanceof Element)) return;
      const tabs = target.closest('.mobile-tabs');
      if (!tabs) return;
      clear();
      // The padding and the home-indicator gap are part of the bar. The column under the
      // finger is the one that was hit, even when the event target is the bar itself.
      tabAt(tabs, event.clientX, event.clientY)?.setAttribute('data-pressed', '');
    },
    { passive: true },
  );
  for (const name of ['pointerup', 'pointercancel']) document.addEventListener(name, clear, { passive: true });
  document.addEventListener(
    'pointerleave',
    (event) => {
      if (event.target instanceof Element && event.target.closest('.mobile-tabs')) clear();
    },
    { capture: true, passive: true },
  );
  window.addEventListener('pagehide', clear);
  // WebKit only shows `:active` on touch when a page listens for touches at all.
  document.addEventListener('touchstart', () => {}, { passive: true });
}

const editing = (target: Element | null) =>
  target !== null &&
  (target.closest("input, textarea, select, [contenteditable]:not([contenteditable='false'])") !== null ||
    ['textbox', 'combobox', 'searchbox'].includes(target.getAttribute('role') || ''));

/**
 * The on-screen keyboard shrinks the visual viewport, but the floating tab bar belongs to the
 * layout viewport: left alone it sits on top of the keyboard, over the results someone is typing
 * to find. It steps aside while an editor holds the keyboard, and the page itself does not move.
 */
export function installKeyboardInset(): void {
  const viewport = window.visualViewport;
  if (!viewport) return;
  const sync = () =>
    document.documentElement.classList.toggle(
      'is-keyboard-open',
      window.innerHeight - viewport.height > 120 && editing(document.activeElement),
    );
  viewport.addEventListener('resize', sync, { passive: true });
  for (const name of ['focusin', 'focusout']) document.addEventListener(name, sync, { passive: true });
}

const pictureFrame = (image: HTMLImageElement) =>
  image.closest('.article-picture, .article-lead, .preview-media, .audio-cover, .media-frame');

// A picture that arrives after a failure, or fails after arriving, must not keep both marks:
// the same source is retried whenever a reader comes back to a page that had no network.
const markPicture = (box: Element, loaded: boolean) => {
  box.classList.toggle('is-loaded', loaded);
  box.classList.toggle('is-error', !loaded);
};

/**
 * Every picture is painted over the placeholder the build inlined behind it, and falls back to
 * its alt text when it never arrives. Two capturing listeners cover the whole document, so no
 * page has to do anything to show a picture honestly.
 */
export function installPictureStates(): void {
  const settle = (event: Event, loaded: boolean) => {
    const target = event.target;
    if (!(target instanceof HTMLImageElement)) return;
    const box = pictureFrame(target);
    if (box) markPicture(box, loaded);
  };
  document.addEventListener('load', (event) => settle(event, true), true);
  document.addEventListener('error', (event) => settle(event, false), true);
}

/** A picture the browser had already finished with never fires either event. */
export function settlePictures(): void {
  for (const image of document.querySelectorAll('img')) {
    if (!image.complete) continue;
    const box = pictureFrame(image);
    if (box) markPicture(box, image.naturalWidth > 0);
  }
}

/**
 * Cmd and Ctrl are the same shortcut wearing the platform's own name; only the name differs, so
 * this decides which one the help shows and nothing about how the keys are handled.
 */
export function markPlatform(): void {
  document.documentElement.dataset.platform = /mac|iphone|ipad|ipod/i.test(
    navigator.userAgentData?.platform || navigator.platform || navigator.userAgent,
  )
    ? 'apple'
    : 'other';
}

/**
 * The feed search sticks where it already sits: under the header, and under a scoped title when
 * the page has one. Both heights are measured. A wrapping header is taller than the single-row
 * fallback, and the title on a source, category or tag page sits between the header and the
 * field. A page without that title clears the extra offset. The observer ends with the page.
 */
export function installFeedChrome(signal: AbortSignal): void {
  const header = document.querySelector('.top');
  if (!(header instanceof HTMLElement)) return;
  const apply = () => {
    if (signal.aborted) return;
    const height = header.getBoundingClientRect().height;
    if (height > 0) document.documentElement.style.setProperty('--top-nav-offset', `${height}px`);
    const heading = document.querySelector('.listhead');
    const block = heading instanceof HTMLElement ? heading.getBoundingClientRect().height : 0;
    document.documentElement.style.setProperty('--feed-heading', `${block}px`);
  };
  apply();
  if (typeof ResizeObserver === 'undefined') return;
  const observer = new ResizeObserver(apply);
  observer.observe(header);
  const heading = document.querySelector('.listhead');
  if (heading instanceof HTMLElement) observer.observe(heading);
  signal.addEventListener('abort', () => observer.disconnect(), { once: true });
}
