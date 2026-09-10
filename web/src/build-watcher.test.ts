import { afterEach, expect, it, vi } from 'vitest';
import { createBuildWatcher } from './build-watcher';

afterEach(() => vi.useRealTimers());
function setup() {
  vi.useFakeTimers();
  const win = Object.assign(new EventTarget(), { navigator: { onLine: true }, location: new URL('https://reader.test/base/') });
  const doc = Object.assign(new EventTarget(), { visibilityState: 'visible' });
  const fetcher = vi.fn<typeof fetch>();
  const apply = vi.fn(async () => {});
  const watcher = createBuildWatcher({ window: win as unknown as Window, document: doc as unknown as Document,
    base: () => win.location.href, enabled: () => true, fetch: fetcher, apply });
  return { watcher, fetcher, apply, win, doc };
}
it('coalesces signals during a check and retries once after the active deployment completes', async () => {
  const { watcher, fetcher, apply } = setup();
  let finish!: (value: Response) => void;
  fetcher.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  fetcher.mockResolvedValue(new Response('{}'));
  const pending = watcher.check();
  expect(watcher.check(true)).toBe(pending);
  expect(watcher.check(true)).toBe(pending);
  finish(new Response('{}'));
  await pending;
  await watcher.check();
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(apply).toHaveBeenCalledTimes(2);
  watcher.dispose();
});
it('stops polling when hidden/offline and ignores a request completed after disposal', async () => {
  const { watcher, fetcher, apply, win, doc } = setup();
  let finish!: (value: Response) => void;
  fetcher.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  watcher.start();
  doc.visibilityState = 'hidden';
  await vi.advanceTimersByTimeAsync(15000);
  win.navigator.onLine = false;
  doc.visibilityState = 'visible';
  win.dispatchEvent(new Event('online'));
  expect(fetcher).toHaveBeenCalledTimes(1);
  watcher.dispose();
  expect(fetcher.mock.calls[0][1]?.signal?.aborted).toBe(true);
  finish(new Response('{}'));
  await vi.advanceTimersByTimeAsync(15000);
  expect(apply).not.toHaveBeenCalled();
});
