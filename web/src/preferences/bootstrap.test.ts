import { describe, expect, it } from "vitest";
import { runBootstrap, type BootstrapSchema } from "./bootstrap.fixture";

const stored = {
  "aggr:theme": "dark",
  "aggr:date-format": "iso",
  "aggr:single-key-shortcuts": "false",
  "aggr:density": "invalid",
  "aggr:scroll-amount": "17",
  "aggr:offline-items": "42",
  "aggr:reading-history": "private"
};

const rejected: unknown[] = [
  null,
  [],
  {},
  "not an object",
  { theme: "dark" },
  { "aggr:theme": "light", "aggr:date-format": "local-time", "aggr:single-key-shortcuts": "false" },
  { version: 2, preferences: { theme: "dark" } },
  { version: "1", preferences: { theme: "dark" } },
  { version: 1, preferences: null },
  { version: 1, preferences: {} },
  { version: 1, preferences: [] },
  { version: 1, preferences: { theme: "purple" } },
  { version: 1, preferences: { "single-key-shortcuts": "false" } },
  { version: 1, preferences: { "scroll-amount": 0 } },
  { version: 1, preferences: { "scroll-amount": 101 } },
  { version: 1, preferences: { "scroll-amount": "10" } },
  { version: 1, preferences: { "offline-items": 1.5 } },
  { version: 1, preferences: { "offline-items": 1001 } },
  { version: 1, preferences: { "paragraph-indent": "true" } },
  { version: 1, preferences: { theme: "dark", history: "private" } },
  { version: 1, preferences: { theme: "dark" }, extra: true },
  { "aggr:theme": "dark", "aggr:single-key-shortcuts": false },
  JSON.parse('{"version":1,"preferences":{"__proto__":{"polluted":true}}}'),
  JSON.parse('{"version":1,"preferences":{"constructor":{"prototype":{"polluted":true}}}}')
];

describe("preferences bootstrap", () => {
  // One run serves the whole scenario: the script must never write, so the last test checks the counter.
  const run = runBootstrap({ stored });
  const { preferences } = run;
  const schema = preferences.schema as BootstrapSchema;

  it("applies the stored theme to the document before paint", () => {
    expect(run.dataset.theme).toBe("dark");
    expect(preferences.values.theme).toBe("dark");
  });

  it("falls back to the default when a stored value is invalid", () => {
    expect(run.dataset.density).toBe("compact");
    expect(preferences.values.density).toBe("compact");
  });

  it("keeps the declared type of every stored value", () => {
    expect(preferences.values["date-format"]).toBe("iso");
    expect(preferences.values["single-key-shortcuts"]).toBe(false);
    expect(preferences.values["scroll-amount"]).toBe(17);
    expect(preferences.values["offline-items"]).toBe(42);
  });

  it("exports only appearance and interaction preferences, with a 50-item feed default", () => {
    expect(Object.keys(preferences.values)).toHaveLength(18);
    expect(preferences.values["feed-page-size"]).toBe("50");
    expect(JSON.stringify(preferences.values)).not.toContain("private");
    expect(preferences.values).not.toHaveProperty("reading-history");
  });

  it("defaults paragraph indentation to off on the document as well", () => {
    expect(preferences.values["paragraph-indent"]).toBe(false);
    expect(run.dataset.paragraphIndent).toBe('false');
  });

  it("writes every attribute-backed preference to the document and nothing else", () => {
    const attributes = Object.values(schema).map(rule => rule.attribute).filter((name): name is string => !!name);
    expect(Object.keys(run.dataset).sort()).toEqual(attributes.sort());
  });

  it("round-trips every option of every preference with its original type", () => {
    const full: Record<string, unknown> = {};
    for (const [key, rule] of Object.entries(schema)) {
      const choices = rule.values ?? [rule.min, rule.max];
      full[key] = choices.at(-1);
      for (const value of choices) {
        expect(preferences.validate({ version: 1, preferences: { [key]: value } })[key], `${key} accepts ${JSON.stringify(value)}`).toBe(value);
      }
    }
    expect(JSON.stringify(preferences.validate(JSON.parse(JSON.stringify({ version: 1, preferences: full }))))).toBe(JSON.stringify(full));
  });

  it.each(rejected)("rejects %j without applying it", payload => {
    expect(() => preferences.validate(payload)).toThrow();
  });

  it("does not pollute prototypes through crafted imports", () => {
    expect(({} as Record<string, unknown>).polluted).toBeUndefined();
    expect(Object.prototype).not.toHaveProperty("polluted");
  });

  it("never writes to storage while loading or validating", () => {
    expect(run.writes()).toBe(0);
    expect(stored["aggr:theme"]).toBe("dark");
  });

  it("still produces valid defaults when storage is unavailable", () => {
    const blocked = runBootstrap({ stored: () => { throw new Error("Storage blocked"); } });
    expect(blocked.preferences.values.theme).toBe("auto");
    expect(blocked.dataset.theme).toBe("auto");
    expect(blocked.writes()).toBe(0);
  });

  it("takes valid site defaults and ignores invalid ones", () => {
    const configured = runBootstrap({ defaults: { theme: "sepia", "offline-items": 7, density: "cosy", "reading-history": "x" } });
    expect(configured.preferences.values.theme).toBe("sepia");
    expect(configured.preferences.values["offline-items"]).toBe(7);
    expect(configured.preferences.values.density).toBe("compact");
    expect(configured.preferences.values).not.toHaveProperty("reading-history");
  });
});
