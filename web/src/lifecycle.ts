export type Dispose = () => void | Promise<void>;

/** A start-up step that failed, recorded on `window.__aggrErrors` for the page (and the browser harness) to read. */
export interface RecordedError { step: string; error: string }

function describeError(error: unknown): string {
  return error instanceof Error ? error.stack || error.message : String(error);
}

/**
 * Run one start-up step without letting its failure stop the others. A throw is recorded on
 * `window.__aggrErrors` (created when missing), reported with `console.error`, and swallowed: the caller gets
 * `undefined` and moves on to the next step.
 */
export function safely<T>(step: string, fn: () => T): T | undefined {
  try { return fn(); }
  catch (error) {
    const page = window as Window & { __aggrErrors?: unknown[] };
    (page.__aggrErrors ??= []).push({ step, error: describeError(error) } satisfies RecordedError);
    console.error(`[aggr] ${step} failed`, error);
    return undefined;
  }
}

/** One owner for the work and components attached to a replaceable page. */
export function createScope() {
  const cancellation = new AbortController();
  const owners: Dispose[] = [];
  let disposed: Promise<void> | undefined;
  return {
    signal: cancellation.signal,
    add(dispose: Dispose) {
      if (cancellation.signal.aborted) throw new Error('Cannot attach to a disposed page');
      owners.push(dispose);
    },
    dispose(): Promise<void> {
      if (disposed) return disposed;
      cancellation.abort();
      const pending = owners.splice(0).reverse().map(dispose => {
        try { return Promise.resolve(dispose()); }
        catch (error) { return Promise.reject(error); }
      });
      disposed = Promise.allSettled(pending).then(results => {
        const failed = results.find(result => result.status === 'rejected');
        if (failed?.status === 'rejected') throw failed.reason;
      });
      return disposed;
    }
  };
}
