type Hook = () => unknown;
type Case = { name: string; body: Hook; before: Hook[]; after: Hook[]; all: Hook[] };
const cases: Case[] = [];
const scopes: { name: string; before: Hook[]; after: Hook[]; all: Hook[] }[] = [{ name: '', before: [], after: [], all: [] }];
const scope = () => scopes[scopes.length - 1];
export const beforeEach = (fn: Hook) => { scope().before.push(fn); };
export const afterEach = (fn: Hook) => { scope().after.push(fn); };
export const beforeAll = (fn: Hook) => { scope().all.push(fn); };
export function describe(name: string, body: Hook): void {
  scopes.push({ name, before: [], after: [], all: [] });
  try { body(); } finally { scopes.pop(); }
}
export const it = Object.assign((name: string, body: Hook) => {
  const parents = [...scopes];
  cases.push({ name: [...scopes.map(item => item.name), name].filter(Boolean).join(' > '), body,
    get before() { return parents.flatMap(item => item.before); },
    get after() { return [...parents].reverse().flatMap(item => [...item.after].reverse()); },
    get all() { return parents.flatMap(item => item.all); } });
}, {
  each: <T>(rows: readonly T[]) => (name: string, body: (...args: any[]) => unknown) => {
    for (const row of rows) {
      const args = Array.isArray(row) ? row : [row];
      let index = 0;
      it(name.replace(/%[sdij]/g, () => String(args[index++])), () => body(...args));
    }
  },
});

const asymmetric = Symbol('matcher');
type Pattern = { [asymmetric]: (value: any) => boolean };
function equal(actual: any, expected: any, partial = false): boolean {
  if (expected && expected[asymmetric]) return expected[asymmetric](actual);
  if (Object.is(actual, expected)) return true;
  if (!actual || !expected || typeof actual !== 'object' || typeof expected !== 'object') return false;
  if (Array.isArray(actual) !== Array.isArray(expected)) return false;
  if (Array.isArray(expected) && actual.length !== expected.length) return false;
  const NodeType = (globalThis as any).Node;
  if (NodeType && (actual instanceof NodeType || expected instanceof NodeType)) return false;
  if (expected instanceof Error) {
    if (!(actual instanceof Error) || actual.name !== expected.name || actual.message !== expected.message || !equal(actual.cause, expected.cause)) return false;
  } else if (actual instanceof Error && !partial) return false;
  if (actual instanceof Date || expected instanceof Date) return actual instanceof Date && expected instanceof Date && +actual === +expected;
  if (actual instanceof RegExp || expected instanceof RegExp) return String(actual) === String(expected);
  if (actual instanceof Set || expected instanceof Set) return actual instanceof Set && expected instanceof Set && actual.size === expected.size && [...expected].every(value => [...actual].some(candidate => equal(candidate, value)));
  if (actual instanceof Map || expected instanceof Map) return actual instanceof Map && expected instanceof Map && actual.size === expected.size && [...expected].every(([key, value]) => [...actual].some(([other, candidate]) => equal(other, key) && equal(candidate, value)));
  const keys = Object.keys(expected);
  return (partial || Object.keys(actual).length === keys.length) && keys.every(key => key in actual && equal(actual[key], expected[key], partial));
}
const repr = (value: any) => { try { return JSON.stringify(value, (_, v) => typeof v === 'bigint' ? String(v) : v) ?? String(value); } catch { return String(value); } };
function assertion(actual: any, negative = false): any {
  const methods: Record<string, (...args: any[]) => boolean> = {
    toBe: expected => Object.is(actual, expected), toEqual: expected => equal(actual, expected),
    toMatchObject: expected => equal(actual, expected, true), toBeNull: () => actual === null,
    toBeUndefined: () => actual === undefined, toBeDefined: () => actual !== undefined, toBeTruthy: () => !!actual,
    toBeInstanceOf: expected => actual instanceof expected, toHaveLength: expected => actual?.length === expected,
    toContain: expected => typeof actual === 'string' ? actual.includes(expected) : [...actual].some(item => Object.is(item, expected)),
    toMatch: expected => typeof expected === 'string' ? actual.includes(expected) : expected.test(actual),
    toBeCloseTo: (expected, precision = 2) => Math.abs(actual - expected) < 0.5 * 10 ** -precision,
    toBeGreaterThan: expected => actual > expected, toBeGreaterThanOrEqual: expected => actual >= expected, toBeLessThan: expected => actual < expected,
    toHaveBeenCalled: () => actual.mock.calls.length > 0,
    toHaveBeenCalledOnce: () => actual.mock.calls.length === 1,
    toHaveBeenCalledTimes: expected => actual.mock.calls.length === expected,
    toHaveBeenCalledWith: (...expected) => actual.mock.calls.some((args: any[]) => equal(args, expected)),
    toHaveBeenCalledExactlyOnceWith: (...expected) => actual.mock.calls.length === 1 && equal(actual.mock.calls[0], expected),
    toHaveBeenLastCalledWith: (...expected) => equal(actual.mock.calls.at(-1), expected),
    toThrow: expected => {
      try { actual(); return false; } catch (error) {
        return expected === undefined || (typeof expected === 'string' ? String((error as Error)?.message ?? error).includes(expected)
          : expected instanceof RegExp ? expected.test(String((error as Error)?.message ?? error))
          : typeof expected === 'function' ? error instanceof expected : equal(error, expected, true));
      }
    },
  };
  return new Proxy({}, { get: (_, key: string) => {
    if (key === 'not') return assertion(actual, !negative);
    if (key === 'resolves' || key === 'rejects') return new Proxy({}, { get: (_, method: string) => async (...args: any[]) => {
      let result: any, rejected = false;
      try { result = await actual; } catch (error) { rejected = true; result = error; }
      if ((key === 'rejects') !== rejected) throw new Error(key === 'rejects' ? 'Expected rejection, promise resolved' : `Expected fulfillment, promise rejected: ${String(result)}`);
      return assertion(key === 'rejects' && method === 'toThrow' ? () => { throw result; } : result, negative)[method](...args);
    } });
    if (!(key in methods)) throw new Error(`Unknown assertion ${key}`);
    return (...args: any[]) => { if (methods[key](...args) === negative) throw new Error(`Expected ${repr(actual)} ${negative ? 'not ' : ''}${key}(${args.map(repr).join(', ')})${actual?.mock ? `; calls=${repr(actual.mock.calls)}` : ''}`); };
  } });
}
export const expect = Object.assign(assertion, {
  any: (type: any): Pattern => ({ [asymmetric]: value => type === Number ? typeof value === 'number' : type === String ? typeof value === 'string' : type === Function ? typeof value === 'function' : value instanceof type }),
  anything: (): Pattern => ({ [asymmetric]: value => value !== null && value !== undefined }),
  objectContaining: (value: any): Pattern => ({ [asymmetric]: actual => equal(actual, value, true) }),
  arrayContaining: (values: any[]): Pattern => ({ [asymmetric]: actual => values.every(expected => actual.some((value: any) => equal(value, expected))) }),
});

type Callable = (...args: any[]) => any;
export type Mock<T extends Callable = Callable> = T & { mock: { calls: Parameters<T>[] }; mockClear(): Mock<T>; mockImplementation(fn: T): Mock<T>; mockReturnValue(value: ReturnType<T>): Mock<T>; mockRejectedValueOnce(error: unknown): Mock<T> };
const restores: (() => void)[] = [];
const globals = new Map<string, PropertyDescriptor | undefined>();
function fn<T extends Callable = Callable>(implementation?: T): Mock<T> {
  let current = implementation;
  const once: Callable[] = [];
  const mock = function(this: unknown, ...args: Parameters<T>) { mock.mock.calls.push(args); return (once.shift() ?? current)?.apply(this, args); } as Mock<T>;
  mock.mock = { calls: [] };
  mock.mockClear = () => { mock.mock.calls.length = 0; return mock; };
  mock.mockImplementation = next => { current = next; return mock; };
  mock.mockReturnValue = value => { current = (() => value) as T; return mock; };
  mock.mockRejectedValueOnce = error => { once.push(() => Promise.reject(error)); return mock; };
  return mock;
}

const native = { setTimeout: globalThis.setTimeout.bind(globalThis), clearTimeout: globalThis.clearTimeout.bind(globalThis), setInterval: globalThis.setInterval.bind(globalThis), clearInterval: globalThis.clearInterval.bind(globalThis), Date };
let fake = false, now = 0, nextTimer = 0;
const timers = new Map<number, { at: number; interval: number; run: () => void }>();
const timerRestores: (() => void)[] = [];
const microtasks = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); };
function setTimer(callback: Callable, delay = 0, interval = 0, args: any[] = []): number {
  const id = ++nextTimer;
  timers.set(id, { at: now + Math.max(0, Number(delay) || 0), interval, run: () => callback(...args) });
  return id;
}
function tick(milliseconds: number): void {
  const target = now + milliseconds;
  let count = 0;
  while (true) {
    const entry = [...timers].filter(([, t]) => t.at <= target).sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
    if (!entry) break;
    if (++count > 10_000) throw new Error('Fake timer did not settle');
    const [id, timer] = entry;
    now = timer.at;
    if (timer.interval) timer.at += timer.interval; else timers.delete(id);
    timer.run();
  }
  now = target;
}
async function tickAsync(milliseconds: number): Promise<void> {
  const target = now + milliseconds;
  await microtasks();
  for (let count = 0; count < 10_000; count++) {
    const entry = [...timers].filter(([, t]) => t.at <= target).sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
    if (!entry) { now = target; await microtasks(); return; }
    const [id, timer] = entry; now = timer.at;
    if (timer.interval) timer.at += timer.interval; else timers.delete(id);
    timer.run(); await microtasks();
  }
  throw new Error('Fake timer did not settle');
}
export const testTools = {
  fn,
  spyOn<T extends object, K extends keyof T>(object: T, key: K): Mock<Extract<T[K], Callable>> {
    const previous = Object.getOwnPropertyDescriptor(object, key);
    const mock = fn(object[key] as Callable);
    Object.defineProperty(object, key, { configurable: true, writable: true, value: mock });
    restores.push(() => previous ? Object.defineProperty(object, key, previous) : Reflect.deleteProperty(object, key));
    return mock as Mock<Extract<T[K], Callable>>;
  },
  mocked: <T extends Callable>(value: T): Mock<T> => value as Mock<T>,
  restoreAllMocks() { for (const restore of restores.splice(0).reverse()) restore(); },
  stubGlobal(name: string, value: any) {
    if (!globals.has(name)) globals.set(name, Object.getOwnPropertyDescriptor(globalThis, name));
    Object.defineProperty(globalThis, name, { configurable: true, writable: true, value });
  },
  unstubAllGlobals() { for (const [name, previous] of globals) { if (previous) Object.defineProperty(globalThis, name, previous); else Reflect.deleteProperty(globalThis, name); } globals.clear(); },
  useFakeTimers(options?: { toFake?: string[] }) {
    this.useRealTimers(); fake = true; now = native.Date.now();
    const selected = options?.toFake ?? ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'Date'];
    const replacements: Record<string, any> = {
      setTimeout: (callback: Callable, delay?: number, ...args: any[]) => setTimer(callback, delay, 0, args), clearTimeout: (id: number) => timers.delete(id),
      setInterval: (callback: Callable, delay = 0, ...args: any[]) => setTimer(callback, delay, Math.max(1, delay), args), clearInterval: (id: number) => timers.delete(id),
      Date: class extends native.Date { constructor(...args: any[]) { super(...(args.length ? args : [now]) as [number]); } static now() { return now; } },
    };
    for (const name of selected) {
      const previous = Object.getOwnPropertyDescriptor(globalThis, name)!;
      timerRestores.push(() => Object.defineProperty(globalThis, name, previous));
      Object.defineProperty(globalThis, name, { configurable: true, writable: true, value: replacements[name] });
    }
  },
  useRealTimers() { for (const restore of timerRestores.splice(0).reverse()) restore(); timers.clear(); fake = false; },
  clearAllTimers() { timers.clear(); }, advanceTimersByTime: tick, advanceTimersByTimeAsync: tickAsync,
  async runAllTimersAsync() { for (let i = 0; timers.size && i < 1000; i++) await tickAsync(Math.max(0, Math.min(...[...timers.values()].map(timer => timer.at)) - now)); if (timers.size) throw new Error('Fake timers did not settle'); },
  async waitFor(body: Hook, options: { timeout?: number; interval?: number } = {}) {
    const deadline = native.Date.now() + (options.timeout ?? 1000);
    while (true) {
      try { return await body(); } catch (error) {
        if (native.Date.now() >= deadline) throw error;
        if (fake) await tickAsync(options.interval ?? 10); else await new Promise(resolve => native.setTimeout(resolve, options.interval ?? 10));
      }
    }
  },
};

declare global { var __aggrTestResult: { cases: string[]; result?: { name: string; error?: string } }; }
export async function run(selected: number | null): Promise<void> {
  globalThis.__aggrTestResult = { cases: cases.map(test => test.name) };
  if (selected === null) return;
  const test = cases[selected];
  if (!test) throw new Error(`Unknown test ${selected}`);
  let error: unknown, failed = false;
  try { for (const setup of test.all) await setup(); for (const setup of test.before) await setup(); await test.body(); }
  catch (failure) { error = failure; failed = true; }
  finally {
    for (const cleanup of test.after) { try { await cleanup(); } catch (failure) { if (!failed) error = failure; failed = true; } }
    testTools.restoreAllMocks(); testTools.unstubAllGlobals(); testTools.useRealTimers();
  }
  await microtasks();
  await new Promise(resolve => native.setTimeout(resolve, 0));
  globalThis.__aggrTestResult.result = { name: test.name, ...(failed ? { error: String((error as Error)?.stack ?? error) } : {}) };
}
