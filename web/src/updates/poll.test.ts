import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { POLL_INTERVAL, Poller, parseBuild } from './poll';

/** A load whose outcome the test decides later. */
function deferred() {
  let resolve!: (value: unknown) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<unknown>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

const build = { app_version: 'app-1', content_version: 'content-1', entries: ['items/a/', 'items/b/'] };

describe('parseBuild', () => {
  it('accepts what the build writes and drops anything else', () => {
    expect(parseBuild(build)).toEqual(build);
    expect(parseBuild({ app_version: 'a', content_version: 'c' })).toEqual({ app_version: 'a', content_version: 'c' });
    expect(parseBuild({ app_version: 'a', content_version: 'c', entries: ['x', 1] })).toEqual({ app_version: 'a', content_version: 'c', entries: ['x'] });
    expect(parseBuild({ app_version: 'a' })).toBeNull();
    expect(parseBuild({ app_version: 1, content_version: 'c' })).toBeNull();
    expect(parseBuild(null)).toBeNull();
    expect(parseBuild('text')).toBeNull();
  });
});

describe('Poller', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('keeps one request in flight and shares it', async () => {
    const pending = deferred();
    const load = vi.fn(() => pending.promise);
    const onBuild = vi.fn();
    const poller = new Poller({ load, onBuild, active: () => true });
    const first = poller.check();
    const second = poller.check();
    expect(second).toBe(first);
    expect(load).toHaveBeenCalledTimes(1);
    pending.resolve(build);
    await first;
    expect(onBuild).toHaveBeenCalledWith(build);
    expect(load).toHaveBeenCalledTimes(1);
  });

  it('queues one more check when asked to insist during a flight', async () => {
    const loads = [deferred(), deferred()];
    const load = vi.fn(() => loads[load.mock.calls.length - 1].promise);
    const onBuild = vi.fn();
    const poller = new Poller({ load, onBuild, active: () => true });
    const first = poller.check();
    void poller.check(true);
    void poller.check(true);
    loads[0].resolve(build);
    await first;
    await Promise.resolve();
    expect(load).toHaveBeenCalledTimes(2);
    loads[1].resolve({ ...build, content_version: 'content-2' });
    await vi.runAllTimersAsync();
    expect(onBuild).toHaveBeenLastCalledWith({ ...build, content_version: 'content-2' });
    expect(onBuild).toHaveBeenCalledTimes(2);
  });

  it('ignores a completion that lands after it was stopped', async () => {
    const pending = deferred();
    const onBuild = vi.fn();
    const poller = new Poller({ load: () => pending.promise, onBuild, active: () => true });
    const check = poller.check();
    poller.stop();
    pending.resolve(build);
    await check;
    expect(onBuild).not.toHaveBeenCalled();
  });

  it('does not ask while inactive, and swallows failures', async () => {
    const load = vi.fn(async () => {
      throw new Error('offline');
    });
    let active = false;
    const onBuild = vi.fn();
    const poller = new Poller({ load, onBuild, active: () => active });
    await poller.check();
    expect(load).not.toHaveBeenCalled();
    active = true;
    await poller.check();
    expect(load).toHaveBeenCalledTimes(1);
    expect(onBuild).not.toHaveBeenCalled();
    // A failed check leaves nothing in flight: the next one asks again.
    await poller.check();
    expect(load).toHaveBeenCalledTimes(2);
  });

  it('checks on the documented interval once started', async () => {
    const load = vi.fn(async () => build);
    const poller = new Poller({ load, onBuild: () => {}, active: () => true });
    poller.start();
    poller.start();
    await vi.advanceTimersByTimeAsync(POLL_INTERVAL * 2);
    expect(load).toHaveBeenCalledTimes(2);
    poller.stop();
    await vi.advanceTimersByTimeAsync(POLL_INTERVAL * 2);
    expect(load).toHaveBeenCalledTimes(2);
  });

  it('gives up on a check that takes too long', async () => {
    const signals: AbortSignal[] = [];
    const poller = new Poller({
      load: (signal) =>
        new Promise((_, reject) => {
          signals.push(signal);
          signal.addEventListener('abort', () => reject(new Error('aborted')));
        }),
      onBuild: () => {},
      active: () => true,
      timeout: 100,
    });
    const check = poller.check();
    await vi.advanceTimersByTimeAsync(100);
    await check;
    expect(signals[0].aborted).toBe(true);
  });
});
