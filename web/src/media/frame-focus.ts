/**
 * A cross-origin frame that holds focus swallows every keydown, and the reader's shortcuts stay
 * dead until the document reloads. YouTube and Vimeo (and an interactive viewer) get the keyboard
 * back on the page. The handoff waits a frame, then retries: the frame often becomes active after
 * the blur event, and Safari may never fire `focus` on the iframe. Twitch draws its own keys, so
 * a frame marked `data-keys="player"` keeps a click's focus. Fullscreen keeps it too.
 *
 * Never focus the page synchronously inside the click or focus that started the frame. That makes
 * iOS treat the tap as unfinished and swallow the next one.
 */

const RETRIES = [40, 160, 400, 1000];

let installed = false;
let restoring = false;
let wave = 0;
let raf = 0;
let timers: ReturnType<typeof setTimeout>[] = [];

function keepsKeys(frame: HTMLIFrameElement): boolean {
  if (frame.dataset.keys === 'player') return true;
  const host = frame.closest('[data-video-provider]');
  return host instanceof HTMLElement && host.dataset.videoProvider === 'twitch';
}

function reclaim(): boolean {
  const active = document.activeElement;
  if (!(active instanceof HTMLIFrameElement) || !active.isConnected) return false;
  if (keepsKeys(active) || document.fullscreenElement === active) return false;
  const page = document.getElementById('content');
  restoring = true;
  if (page instanceof HTMLElement) page.focus({ preventScroll: true });
  else active.blur();
  restoring = false;
  return document.activeElement !== active;
}

function clearPending(): void {
  if (raf) cancelAnimationFrame(raf);
  raf = 0;
  for (const timer of timers) clearTimeout(timer);
  timers = [];
}

/** Try again across a short window, coalesced so a burst of blur and focus events is one wave. */
function schedule(): void {
  if (restoring) return;
  clearPending();
  const token = ++wave;
  const run = () => {
    if (token !== wave || restoring) return;
    if (!reclaim()) return;
    wave += 1;
    clearPending();
  };
  if (typeof requestAnimationFrame === 'function') raf = requestAnimationFrame(run);
  else run();
  for (const delay of RETRIES) timers.push(setTimeout(run, delay));
}

/** Watch the document. One install covers players created later and interactive viewers. */
export function installFrameFocus(): void {
  if (installed) return;
  installed = true;
  document.addEventListener(
    'focusin',
    (event) => {
      if (event.target instanceof HTMLIFrameElement && !keepsKeys(event.target)) schedule();
    },
    true,
  );
  window.addEventListener('blur', () => {
    const active = document.activeElement;
    if (active instanceof HTMLIFrameElement && keepsKeys(active)) return;
    schedule();
  });
  document.addEventListener('fullscreenchange', () => {
    if (!document.fullscreenElement) schedule();
  });
}

/** Drop a pending handoff. The next blur or focus schedules a new one. */
export function resetFrameFocus(): void {
  wave += 1;
  clearPending();
}
