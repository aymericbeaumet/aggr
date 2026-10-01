/**
 * The reader's notion of now, for relative dates and age bands. It is null until started: a
 * server render, and a document with no running app, show the build's own dates and bands.
 */
class Clock {
  now = $state<number | null>(null);
}

export const clock = new Clock();

/** Tick every `interval` while the tab is visible, and on each return to it. Returns a stop. */
export function startClock(interval = 60_000, doc: Document = document): () => void {
  clock.now = Date.now();
  const tick = () => {
    if (!doc.hidden) clock.now = Date.now();
  };
  const timer = setInterval(tick, interval);
  doc.addEventListener('visibilitychange', tick);
  return () => {
    clearInterval(timer);
    doc.removeEventListener('visibilitychange', tick);
  };
}
