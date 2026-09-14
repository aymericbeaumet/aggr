import { describe, expect, it } from 'vitest';
import { createScope } from './lifecycle';

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
