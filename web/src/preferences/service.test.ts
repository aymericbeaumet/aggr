import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { describe, expect, it, vi } from "vitest";
import type { Preferences } from "../contracts";
import { createPreferencesService } from "./service";
import { fields, preferenceDescription } from "./presentation";

function fixture() {
  const stored = new Map<string, string>();
  const meta = { setAttribute: vi.fn() };
  const document = { documentElement: { dataset: {} as Record<string, string> }, querySelector: () => meta };
  const localStorage = { getItem: (key: string) => stored.get(key) ?? null, setItem: (key: string, value: string) => { stored.set(key, value); } };
  const template = readFileSync(new URL("../../../themes/default/templates/base.html", import.meta.url), "utf8");
  const script = template.split('<script id="aggr-preferences">')[1].split("</script>")[0].replace("{{ site.preferences | json }}", '{"theme":"sepia","offline-items":7}');
  const global: { AGGRPreferences?: Preferences } = {};
  runInNewContext(script, { window: global, document, localStorage });
  const bootstrap = global.AGGRPreferences!;
  const window = Object.assign(new EventTarget(), {
    localStorage,
    btoa, atob,
    location: { href: "https://example.test/aggr/preferences/", replace: vi.fn() },
    history: { state: null, replaceState: vi.fn() },
    getComputedStyle: vi.fn(() => ({ getPropertyValue: (): string => " #123456 " })),
  });
  const onApply = vi.fn();
  const service = createPreferencesService(bootstrap, { base: () => "https://example.test/aggr/", document: document as unknown as Document, window: window as unknown as Window, onApply, replaceLocation: url => { window.history.replaceState(null, "", url); } });
  return { service, bootstrap, document, window, stored, onApply, meta };
}
const file = (preferences: unknown) => ({ size: 50, text: async () => JSON.stringify(preferences) });

describe("preferences service", () => {
  it("reuses unchanged application preferences without DOM writes or synchronous theme reads", () => {
    const f = fixture();
    f.service.apply(f.bootstrap.values, false);
    expect(f.window.getComputedStyle).toHaveBeenCalledTimes(1);
    expect(f.meta.setAttribute).toHaveBeenLastCalledWith("content", "#123456");
    const write = vi.fn();
    f.document.documentElement.dataset = new Proxy(f.document.documentElement.dataset, {
      set(target, key, value) { write(key, value); return Reflect.set(target, key, value); },
    });
    const subscription = vi.fn();
    f.service.subscribe(subscription);
    subscription.mockClear();
    f.service.apply(f.bootstrap.values, false);
    expect(write).not.toHaveBeenCalled();
    expect(subscription).not.toHaveBeenCalled();
    expect(f.window.getComputedStyle).toHaveBeenCalledTimes(1);
    expect(f.onApply).toHaveBeenCalledTimes(2);
    f.service.apply({ "offline-items": 8 });
    expect(f.window.getComputedStyle).toHaveBeenCalledTimes(1);
    expect(f.service.getSnapshot().values["offline-items"]).toBe(8);
    f.service.apply({ theme: "dark" });
    expect(f.window.getComputedStyle).toHaveBeenCalledTimes(2);
    expect(write).toHaveBeenCalledWith("theme", "dark");
  });

  it("refreshes browser color for system theme changes and stops after disposal", () => {
    const f = fixture();
    f.service.apply({ theme: "auto" });
    f.window.getComputedStyle.mockReturnValue({ getPropertyValue: () => "#abcdef" });
    f.service.refreshTheme();
    expect(f.meta.setAttribute).toHaveBeenLastCalledWith("content", "#abcdef");
    expect(f.window.getComputedStyle).toHaveBeenCalledTimes(2);
    f.service.dispose();
    f.service.refreshTheme();
    expect(f.window.getComputedStyle).toHaveBeenCalledTimes(2);
  });

  it("offers every supported preference and only values accepted by the bootstrap", () => {
    const f = fixture();
    expect(Object.keys(fields).sort()).toEqual(Object.keys(f.bootstrap.schema).sort());
    for (const [key, field] of Object.entries(fields)) {
      for (const option of field.options) expect(f.bootstrap.valid(key, option.value)).toBe(true);
    }
    expect(preferenceDescription("theme", "auto")).toBe("Theme: System");
    expect(preferenceDescription("single-key-shortcuts", false)).toBe("Enable keyboard shortcuts: Off");
  });
  it("uses the real bootstrap validator and applies imports only after review", async () => {
    const f = fixture();
    await f.service.importFile(file({ version: 1, preferences: { theme: "dark", "offline-items": 12 } }));
    expect(f.bootstrap.values.theme).toBe("sepia");
    expect(f.stored.size).toBe(0);
    expect(f.service.getSnapshot().pending).toEqual({ theme: "dark", "offline-items": 12 });
    f.service.applyImport();
    expect(f.bootstrap.values.theme).toBe("dark");
    expect(f.document.documentElement.dataset.theme).toBe("dark");
    expect(f.stored.get("aggr:offline-items")).toBe("12");
    expect(f.onApply).toHaveBeenCalledWith(true);
    expect(f.service.getSnapshot().pending).toBeNull();
  });

  it.each([
    { "aggr:theme": "dark" },
    { theme: "dark" },
    { preferences: { theme: "dark" } },
    { version: 1, preferences: { "aggr:theme": "dark" } },
    { version: 2, preferences: { theme: "dark" } },
    { version: 1, preferences: { "reading-history": "x" } },
    { version: 1, preferences: { "offline-items": 1001 } },
    { version: 1, preferences: { theme: "dark", extra: true } },
  ])("rejects invalid imports without partially applying them: %j", async payload => {
    const f = fixture();
    await f.service.importFile(file(payload));
    expect(f.service.getSnapshot().pending).toBeNull();
    expect(f.service.getSnapshot().status).toContain("Nothing was changed");
    expect(f.bootstrap.values.theme).toBe("sepia");
    expect(f.stored.size).toBe(0);
  });

  it("keeps session preferences usable when storage is unavailable", () => {
    const f = fixture();
    f.window.localStorage.setItem = () => { throw new Error("blocked"); };
    f.service.apply({ theme: "dark" });
    expect(f.bootstrap.values.theme).toBe("dark");
    expect(f.service.getSnapshot().status).toContain("session");
    f.service.reset();
    expect(f.bootstrap.values.theme).toBe("sepia");
    expect(f.bootstrap.values["offline-items"]).toBe(7);
    expect(f.service.getSnapshot().status).toContain("storage is unavailable");
  });

  it("restores site defaults without clearing unrelated history", () => {
    const f = fixture();
    f.stored.set("aggr:history", "preserved");
    f.service.apply({ theme: "dark" });
    f.service.reset();
    expect(f.bootstrap.values.theme).toBe("sepia");
    expect(f.stored.get("aggr:history")).toBe("preserved");
  });

  it("synchronizes cross-tab preferences and detaches its subscription", () => {
    const f = fixture();
    f.stored.set("aggr:theme", "dark");
    f.window.dispatchEvent(Object.assign(new Event("storage"), { key: "aggr:theme" }));
    expect(f.bootstrap.values.theme).toBe("dark");
    expect(f.onApply).toHaveBeenCalledWith(false);
    f.service.dispose();
    f.stored.set("aggr:theme", "light");
    f.window.dispatchEvent(Object.assign(new Event("storage"), { key: "aggr:theme" }));
    expect(f.bootstrap.values.theme).toBe("dark");
  });

  it("ignores stale file reads after another import or page disposal", async () => {
    const f = fixture();
    let resolve!: (raw: string) => void;
    const reading = f.service.importFile({ size: 10, text: () => new Promise<string>(done => { resolve = done; }) });
    await f.service.importFile(file({ version: 1, preferences: { theme: "light" } }));
    resolve(JSON.stringify({ version: 1, preferences: { theme: "dark" } }));
    await reading;
    expect(f.service.getSnapshot().pending).toEqual({ theme: "light" });
    const obsolete = f.service.importFile({ size: 10, text: () => new Promise<string>(done => { resolve = done; }) });
    f.service.cancelFileRead();
    resolve("not valid JSON");
    await obsolete;
    expect(f.service.getSnapshot().pending).toEqual({ theme: "light" });
  });

  it("bounds file reads and round trips versioned fragment links", async () => {
    const f = fixture();
    const text = vi.fn(async () => "{}");
    await f.service.importFile({ size: 16385, text });
    expect(text).not.toHaveBeenCalled();
    f.service.apply({ theme: "dark" });
    f.window.location.href = f.service.link();
    f.service.apply({ theme: "light" });
    f.service.importLocation("preferences");
    expect(f.bootstrap.values.theme).toBe("light");
    expect(f.service.getSnapshot().pending?.theme).toBe("dark");
    expect(f.window.history.replaceState).toHaveBeenCalled();
    f.service.applyImport();
    expect(f.bootstrap.values.theme).toBe("dark");
  });

  it("does not import obsolete query-string transfer links", () => {
    const f = fixture();
    f.window.location.href = "https://example.test/aggr/preferences/?aggr-state=" + btoa(JSON.stringify({ version: 1, preferences: { theme: "dark" } }));
    f.service.importLocation("preferences");
    expect(f.service.getSnapshot().pending).toBeNull();
    expect(f.bootstrap.values.theme).toBe("sepia");
    expect(f.window.history.replaceState).not.toHaveBeenCalled();
  });
});
