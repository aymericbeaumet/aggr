import { describe, expect, it } from "vitest";
import { ageBand, entryStateKey, mergeNewEntries, remainingEntries, resolveEntries, scopedEntries, uniqueEntries } from "./entries";

const base = "https://reader.test/nested/";

describe("new entry detection", () => {
  it("namespaces session state by the site path", () => {
    expect(entryStateKey("new-entries", base)).toBe("aggr:new-entries:%2Fnested%2F");
    expect(entryStateKey("last-seen-entry", "https://reader.test/")).toBe("aggr:last-seen-entry:%2F");
  });

  it("treats everything above the last seen head as new and keeps earlier pending entries", () => {
    const current = ["c", "b", "a"];
    expect(mergeNewEntries("a", current, [])).toEqual(["c", "b"]);
    expect(mergeNewEntries("b", current, ["z"])).toEqual(["z", "c"]);
    expect(mergeNewEntries("c", current, ["c"])).toEqual(["c"]);
  });

  it("highlights nothing on a first visit or an empty list, and everything when the head has scrolled off", () => {
    expect(mergeNewEntries(null, ["c", "b"], [])).toEqual([]);
    expect(mergeNewEntries("", ["c", "b"], ["kept"])).toEqual(["kept"]);
    expect(mergeNewEntries("a", [], ["kept"])).toEqual(["kept"]);
    expect(mergeNewEntries("gone", ["c", "b"], ["b"])).toEqual(["b", "c"]);
  });

  it("resolves entries against the site and drops duplicates and garbage", () => {
    expect(resolveEntries(["items/one/", base + "items/one/", "items/two/", "http://[bad"], base)).toEqual([base + "items/one/", base + "items/two/"]);
    expect(uniqueEntries(["a", "b", "a"])).toEqual(["a", "b"]);
  });

  it("keeps only manifest entries inside the site", () => {
    expect(scopedEntries(["items/one/", base + "items/two/", "https://elsewhere.test/x/", "/outside/", 42, null, "http://[bad"], base))
      .toEqual(["items/one/", base + "items/two/"]);
  });

  it("removes acknowledged entries from the pending list", () => {
    expect(remainingEntries(["a", "b", "c"], new Set(["b"]))).toEqual(["a", "c"]);
    expect(remainingEntries(["a"], new Set())).toEqual(["a"]);
  });
});

describe("age bands", () => {
  const now = Date.parse("2026-09-16T12:00:00Z");
  const hours = (count: number) => new Date(now - count * 60 * 60 * 1000).toISOString();

  it("bands published times at one, three and twenty-four hours", () => {
    expect(ageBand(hours(0.5), now)).toBe("fresh");
    expect(ageBand(hours(1), now)).toBe("h1");
    expect(ageBand(hours(2.9), now)).toBe("h1");
    expect(ageBand(hours(3), now)).toBe("h3");
    expect(ageBand(hours(23.9), now)).toBe("h3");
    expect(ageBand(hours(24), now)).toBe("h24");
  });

  it("treats future and unparsable times as fresh and oldest respectively", () => {
    expect(ageBand(hours(-5), now)).toBe("fresh");
    expect(ageBand("", now)).toBe("h24");
    expect(ageBand("not a date", now)).toBe("h24");
  });
});
