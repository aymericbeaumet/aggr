/**
 * Whether the browser has a network, as it reports it. `true` until started (a server render
 * and a document without the app assume a network), then kept current by the window's
 * `online` and `offline` events.
 */
class Connection {
  online = $state(true);
  /** The reader asked for less data (`Save-Data`); read once at start. */
  saveData = false;

  /** Follow the window's network state. Returns a stop. */
  start(win: Window = window): () => void {
    this.online = win.navigator.onLine;
    this.saveData = win.navigator.connection?.saveData === true;
    const on = () => (this.online = true);
    const off = () => (this.online = false);
    win.addEventListener('online', on);
    win.addEventListener('offline', off);
    return () => {
      win.removeEventListener('online', on);
      win.removeEventListener('offline', off);
    };
  }
}

export const connection = new Connection();
