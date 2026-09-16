import { afterEach, describe, expect, it, vi } from 'vitest';
import { createScope, safely } from './lifecycle';

describe('page ownership', () => {
  it('cancels work immediately and waits for every component before replacement', async () => {
    const scope = createScope();
    const events: string[] = [];
    let finish!: () => void;
    scope.signal.addEventListener('abort', () => events.push('abort'));
    scope.add(() => { events.push('listeners removed'); });
    scope.add(() => new Promise<void>(resolve => { events.push('unmount'); finish = resolve; }));
    const disposed = scope.dispose().then(() => events.push('replace'));
    expect(scope.signal.aborted).toBe(true);
    expect(events).toEqual(['abort', 'unmount', 'listeners removed']);
    finish();
    await disposed;
    expect(events.at(-1)).toBe('replace');
  });

  it('cleans all owners even if one fails and never disposes twice', async () => {
    const scope = createScope();
    let cleaned = 0;
    scope.add(() => { cleaned++; });
    scope.add(() => { throw new Error('failed unmount'); });
    const first = scope.dispose();
    expect(scope.dispose()).toBe(first);
    await expect(first).rejects.toThrow('failed unmount');
    expect(cleaned).toBe(1);
    expect(() => scope.add(() => {})).toThrow('disposed');
  });
});

describe('guarded start-up steps', () => {
  afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

  it('records a failing step on the page and continues with the next one', () => {
    const page: { __aggrErrors?: unknown[] } = {};
    vi.stubGlobal('window', page);
    const reported = vi.spyOn(console, 'error').mockImplementation(() => {});
    const order: string[] = [];
    expect(safely('search', () => { order.push('search'); throw new Error('mount failed'); })).toBeUndefined();
    expect(safely('media', () => { order.push('media'); return 42; })).toBe(42);
    expect(order).toEqual(['search', 'media']);
    expect(page.__aggrErrors).toHaveLength(1);
    const [recorded] = page.__aggrErrors as { step: string; error: string }[];
    expect(recorded.step).toBe('search');
    expect(recorded.error).toContain('mount failed');
    expect(reported).toHaveBeenCalledWith('[aggr] search failed', expect.any(Error));
  });

  it('appends to an existing error list and describes non-Error throws', () => {
    const page = { __aggrErrors: ['earlier'] as unknown[] };
    vi.stubGlobal('window', page);
    vi.spyOn(console, 'error').mockImplementation(() => {});
    safely('boot', () => { throw 'plain failure'; });
    expect(page.__aggrErrors).toEqual(['earlier', { step: 'boot', error: 'plain failure' }]);
  });
});
