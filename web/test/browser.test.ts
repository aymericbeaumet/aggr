import { afterEach, beforeEach, describe, expect, it, testTools } from './browser';

describe('browser test harness', () => {
  it('checks deep values and asymmetric partial patterns', () => {
    expect({ nested: { values: [1, 2] }, extra: true }).toMatchObject({ nested: { values: expect.arrayContaining([2]) } });
    expect({ fn: () => {}, value: 1 }).toEqual({ fn: expect.any(Function), value: 1 });
    expect(new Set([1, 2])).toEqual(new Set([1, 2]));
    expect(new Map([['a', 1]])).toEqual(new Map([['a', 1]]));
    expect(() => expect({ a: 1 }).toEqual({ a: 2 })).toThrow(/Expected/);
    expect(() => expect(1).not.toBe(1)).toThrow(/Expected/);
    expect(() => expect([]).toEqual({})).toThrow(/Expected/);
    expect(() => expect(new Array(2)).toEqual([])).toThrow(/Expected/);
    expect(() => expect(new Array(1)).toEqual([undefined])).toThrow(/Expected/);
    expect(() => expect(new Error('one')).toEqual(new Error('two'))).toThrow(/Expected/);
    expect(new Error('one')).toEqual(new Error('one'));
  });
  it('distinguishes fulfilled and rejected asynchronous assertions', async () => {
    await expect(Promise.resolve(2)).resolves.toBe(2);
    await expect(Promise.reject(new Error('broken'))).rejects.toThrow('broken');
    await expect(Promise.resolve(2).then(async value => { await expect(Promise.resolve(value)).rejects.toThrow(); })).rejects.toThrow('Expected rejection');
  });
  it('records arguments, preserves method receivers and restores spies', () => {
    const object = { value: 4, add(amount: number) { return this.value + amount; } };
    const spy = testTools.spyOn(object, 'add');
    expect(object.add(3)).toBe(7);
    expect(spy).toHaveBeenCalledExactlyOnceWith(3);
    spy.mockReturnValue(9);
    expect(object.add(1)).toBe(9);
    testTools.restoreAllMocks();
    expect(object.add(1)).toBe(5);
  });
  it('advances fake timers in deadline order including promise continuations', async () => {
    testTools.useFakeTimers();
    const values: string[] = [];
    setTimeout(() => { values.push('first'); Promise.resolve().then(() => setTimeout(() => values.push('last'), 5)); }, 10);
    setTimeout(() => values.push('middle'), 12);
    await testTools.advanceTimersByTimeAsync(20);
    expect(values).toEqual(['first', 'middle', 'last']);
  });
  it('clears intervals and restores stubbed globals', async () => {
    testTools.useFakeTimers();
    let count = 0;
    const timer = setInterval(() => count++, 5);
    await testTools.advanceTimersByTimeAsync(10);
    clearInterval(timer);
    await testTools.advanceTimersByTimeAsync(10);
    expect(count).toBe(2);
    const original = globalThis.fetch;
    testTools.stubGlobal('fetch', testTools.fn());
    expect(globalThis.fetch).not.toBe(original);
    testTools.unstubAllGlobals();
    expect(globalThis.fetch).toBe(original);
  });
});

describe('suite hooks registered after a test', () => {
  let state = 0;
  it('observes setup and scoped cleanup', () => expect(state).toBe(1));
  beforeEach(() => { state = 1; });
  afterEach(() => { expect(state).toBe(1); state = 0; });
});
