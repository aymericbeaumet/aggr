import { afterEach, describe, expect, it, vi } from "vitest";
import { createOfflineClient } from "./offline-client";

afterEach(() => vi.useRealTimers());

function fixture(pwa = true) {
  vi.useFakeTimers();
  const first = { postMessage: vi.fn() };
  let resolveReady!: (value: unknown) => void;
  let resolveRegistration!: (value: unknown) => void;
  const registration = { scope: "https://example.test/aggr/", update: vi.fn(async () => {}), unregister: vi.fn(async () => true) };
  const worker = Object.assign(new EventTarget(), {
    controller: first,
    ready: new Promise(done => { resolveReady = done; }),
    register: vi.fn(() => new Promise(done => { resolveRegistration = done; })),
    getRegistration: vi.fn(async () => registration),
  });
  const navigator = { serviceWorker: worker, onLine: true };
  const document = Object.assign(new EventTarget(), { visibilityState: "visible" });
  const window = Object.assign(new EventTarget(), {
    location: { protocol: "https:" },
    caches: { keys: vi.fn(async () => ["aggr:%2Faggr%2F:articles", "aggr:%2Fother%2F:articles", "unrelated"]), delete: vi.fn(async () => true) },
  });
  const onBuild = vi.fn(), onStatus = vi.fn(), onControllerChange = vi.fn();
  let count = 7;
  const client = createOfflineClient({ base: () => "https://example.test/aggr/", pwa: () => pwa, kind: () => "river", count: () => count, onBuild, onStatus, onControllerChange }, {
    window: window as unknown as Window, document: document as unknown as Document, navigator: navigator as unknown as Navigator,
  });
  return { client, first, worker, registration, navigator, document, window, onBuild, onStatus, onControllerChange, setCount(value: number) { count = value; }, ready() { resolveReady(registration); }, registered() { resolveRegistration(registration); } };
}

describe("offline client lifecycle", () => {
  it("deduplicates preferences and requests build/status once ready", async () => {
    const f = fixture();
    f.client.start(); f.client.start();
    expect(f.worker.register).toHaveBeenCalledOnce();
    f.ready(); await Promise.resolve();
    expect(f.first.postMessage.mock.calls.map(([message]) => message.type)).toEqual(["AGGR_OFFLINE_CONFIG", "AGGR_GET_BUILD", "AGGR_OFFLINE_GET_STATUS"]);
    f.client.configure();
    expect(f.first.postMessage).toHaveBeenCalledTimes(3);
    f.setCount(12); f.client.configure();
    expect(f.first.postMessage).toHaveBeenLastCalledWith({ type: "AGGR_OFFLINE_CONFIG", count: 12 });
    f.client.dispose();
  });

  it("accepts only the controlling worker's messages and configures its replacement", () => {
    const f = fixture(); f.client.start();
    const data = { type: "AGGR_BUILD", app_version: "new" };
    f.worker.dispatchEvent(Object.assign(new Event("message"), { source: {}, data }));
    expect(f.onBuild).not.toHaveBeenCalled();
    f.worker.dispatchEvent(Object.assign(new Event("message"), { source: f.first, data }));
    expect(f.onBuild).toHaveBeenCalledWith(data);
    const status = { type: "AGGR_OFFLINE_STATUS", requested: 7, total: 7, saved: [], failed: 0, downloading: true };
    f.worker.dispatchEvent(Object.assign(new Event("message"), { source: f.first, data: status }));
    expect(f.onStatus).toHaveBeenCalledWith(status);
    const next = { postMessage: vi.fn() };
    f.worker.controller = next;
    f.worker.dispatchEvent(new Event("controllerchange"));
    expect(f.onControllerChange).toHaveBeenCalledOnce();
    expect(next.postMessage).toHaveBeenCalledWith({ type: "AGGR_OFFLINE_CONFIG", count: 7 });
    f.worker.dispatchEvent(Object.assign(new Event("message"), { source: f.first, data }));
    expect(f.onBuild).toHaveBeenCalledTimes(1);
    f.client.dispose();
  });

  it("ignores ready/register continuations and messages after disposal", async () => {
    const f = fixture(); f.client.start(); f.client.dispose();
    f.ready(); f.registered(); await Promise.resolve();
    await vi.advanceTimersByTimeAsync(180000);
    f.window.dispatchEvent(new Event("online"));
    f.worker.dispatchEvent(new Event("controllerchange"));
    expect(f.first.postMessage).not.toHaveBeenCalled();
    expect(f.registration.update).not.toHaveBeenCalled();
    expect(f.onControllerChange).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("polls only while online and visible, prevents overlapping updates, and removes timers", async () => {
    const f = fixture(); f.client.start(); f.registered(); await Promise.resolve();
    f.document.visibilityState = "hidden";
    await vi.advanceTimersByTimeAsync(60000);
    expect(f.registration.update).not.toHaveBeenCalled();
    f.document.visibilityState = "visible";
    let finish!: () => void;
    f.registration.update.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    f.document.dispatchEvent(new Event("visibilitychange"));
    f.window.dispatchEvent(new Event("online"));
    await vi.advanceTimersByTimeAsync(120000);
    expect(f.registration.update).toHaveBeenCalledOnce();
    finish(); await Promise.resolve(); await Promise.resolve();
    f.client.dispose();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("cleans only this site's caches and registration when PWA is disabled", async () => {
    const f = fixture(false); f.client.start();
    await Promise.resolve(); await Promise.resolve();
    expect(f.registration.unregister).toHaveBeenCalledOnce();
    expect(f.window.caches.delete.mock.calls).toEqual([["aggr:%2Faggr%2F:articles"]]);
    expect(f.worker.register).not.toHaveBeenCalled();
    f.client.dispose();
  });

  it("leaves an unrelated parent worker registered when PWA is disabled", async () => {
    const f = fixture(false); f.registration.scope = "https://example.test/";
    f.client.start(); await Promise.resolve();
    expect(f.registration.unregister).not.toHaveBeenCalled();
    f.client.dispose();
  });
});
