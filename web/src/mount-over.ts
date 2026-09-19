import { unmount } from 'svelte';

/** What Svelte's `mount()` returns: the component's exports. */
type Mounted = Record<string, unknown>;

export interface Overlay<T extends Mounted[]> {
  /** The mounted components, in mount order. */
  mounted: T;
  /** The server-rendered children that were replaced, ready for `target.replaceChildren(...fallback)`. */
  fallback: ChildNode[];
  /** Unmount every component, then put the server-rendered children back. */
  restore(): Promise<void>;
}

/**
 * Replace the server-rendered children of `target` with mounted components.
 *
 * The children are snapshotted once before the target is cleared. When a mount throws, whatever was already
 * mounted is unmounted, the snapshot is put back and the error is rethrown, so the page keeps its
 * no-JavaScript markup instead of an empty node.
 */
export function mountOver<T extends Mounted[]>(target: ParentNode, mounts: { [K in keyof T]: () => T[K] }): Overlay<T> {
  const fallback = Array.from(target.childNodes);
  target.replaceChildren();
  const mounted = [] as Mounted[] as T;
  try {
    for (const mount of mounts as Array<() => Mounted>) mounted.push(mount());
  } catch (error) {
    for (const component of mounted) void Promise.resolve(unmount(component)).catch(() => {});
    target.replaceChildren(...fallback);
    throw error;
  }
  return {
    mounted, fallback,
    async restore() {
      await Promise.all(mounted.map(component => unmount(component)));
      target.replaceChildren(...fallback);
    }
  };
}
