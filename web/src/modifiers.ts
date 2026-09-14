export function installShiftHover(document: Document, window: Window): () => void {
  const listeners = new AbortController();
  const options = { capture: true, signal: listeners.signal };
  const setHeld = (held: boolean) => document.documentElement.classList.toggle('is-shift-held', held);
  const dispose = () => { listeners.abort(); setHeld(false); };
  document.addEventListener('keydown', event => setHeld(event.shiftKey), options);
  document.addEventListener('keyup', event => setHeld(event.shiftKey), options);
  document.addEventListener('pointerover', event => setHeld(event.shiftKey), options);
  document.addEventListener('visibilitychange', () => { if (document.hidden) setHeld(false); }, options);
  window.addEventListener('blur', () => setHeld(false), options);
  window.addEventListener('pagehide', event => {
    setHeld(false);
    // A page restored from the back/forward cache keeps its existing runtime listeners.
    if (!event.persisted) dispose();
  }, options);
  return dispose;
}
