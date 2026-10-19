/**
 * Everything a page sets up for itself (listeners on its own content, timers, observers) belongs
 * to the page rather than the document: moving to another page in place ends it through this.
 */
let controller: AbortController | null = null;

export const pageScope = {
  get signal(): AbortSignal {
    return (controller ??= new AbortController()).signal;
  },
  /** Whether the page on screen has been left. */
  get aborted(): boolean {
    return controller?.signal.aborted ?? false;
  },
  /** End the page on screen. */
  abort(): void {
    controller?.abort();
  },
  /** Start the page arriving; its signal is the one page setup runs under. */
  renew(): AbortSignal {
    if (!controller?.signal.aborted) controller?.abort();
    controller = new AbortController();
    return (controller ??= new AbortController()).signal;
  },
};
