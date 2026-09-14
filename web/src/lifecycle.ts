export type Dispose = () => void | Promise<void>;

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
