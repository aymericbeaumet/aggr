import { describe, expect, it } from "vitest";
import { articleNavigationDirection, articleNavigationTarget, externalShortcutKey, externalShortcutTarget, gotoRoute, lineHeight, scrollDistance, scrollShortcut } from "./shortcuts";

const base = "https://reader.test/nested/";

describe("goto chords", () => {
  it("routes letters, digits and the top of the page", () => {
    expect(gotoRoute("g", [], base)).toEqual({ type: "first" });
    expect(gotoRoute("f", [], base)).toEqual({ type: "navigate", url: base });
    expect(gotoRoute("i", [], base)).toEqual({ type: "navigate", url: base });
    expect(gotoRoute("l", [], base)).toEqual({ type: "navigate", url: base + "browse/" });
    expect(gotoRoute("p", [], base)).toEqual({ type: "navigate", url: base + "preferences/" });
    expect(gotoRoute("2", ["items/one/", "items/two/"], base)).toEqual({ type: "navigate", url: base + "items/two/" });
  });

  it("ignores unknown keys, missing entries and inherited object names", () => {
    expect(gotoRoute("x", [], base)).toBeNull();
    expect(gotoRoute("3", ["items/one/"], base)).toBeNull();
    expect(gotoRoute("0", ["items/one/"], base)).toBeNull();
    expect(gotoRoute("constructor", [], base)).toBeNull();
    expect(gotoRoute("toString", [], base)).toBeNull();
  });
});

describe("external shortcuts", () => {
  const networks = [
    { name: "hackernews", shortcut: "H", url: "https://news.ycombinator.com/from?site={url}" },
    { name: "reddit", shortcut: "R", url: "https://www.reddit.com/search/?q={title}%20{url}" },
    { name: "lobsters", url: "https://lobste.rs/search?q={url}" }
  ];
  const article = { original: "https://publisher.test/story", link: "https://publisher.test/story?a=1&b=2", title: "Hello & welcome", discussions: [{ name: "hackernews", href: "https://news.ycombinator.com/item?id=1" }] };

  it("accepts only single upper-case keys", () => {
    expect(externalShortcutKey("O")).toBe(true);
    expect(externalShortcutKey("o")).toBe(false);
    expect(externalShortcutKey("Enter")).toBe(false);
    expect(externalShortcutKey("1")).toBe(true);
  });

  it("opens the original, a rendered discussion link, or the network's search template", () => {
    expect(externalShortcutTarget("O", article, networks)).toBe("https://publisher.test/story");
    expect(externalShortcutTarget("O", { ...article, original: null }, networks)).toBeNull();
    expect(externalShortcutTarget("H", article, networks)).toBe("https://news.ycombinator.com/item?id=1");
    expect(externalShortcutTarget("R", article, networks)).toBe("https://www.reddit.com/search/?q=Hello%20%26%20welcome%20https%3A%2F%2Fpublisher.test%2Fstory%3Fa%3D1%26b%3D2");
    expect(externalShortcutTarget("L", article, networks)).toBeNull();
    expect(externalShortcutTarget("h", article, networks)).toBeNull();
    expect(externalShortcutTarget("1", article, networks)).toBeNull();
  });
});

describe("scroll keys", () => {
  const chord = (key: string, modifiers: Partial<Omit<import("./shortcuts").KeyChord, "key">> = {}) =>
    ({ key, altKey: false, ctrlKey: false, metaKey: false, shiftKey: false, ...modifiers });

  it("recognises d/u and ctrl-e/ctrl-y unless typing or another chord is held", () => {
    expect(scrollShortcut(chord("d"), false, true)).toEqual({ key: "d", lineScroll: false });
    expect(scrollShortcut(chord("U"), false, true)).toEqual({ key: "u", lineScroll: false });
    expect(scrollShortcut(chord("e", { ctrlKey: true }), false, false)).toEqual({ key: "e", lineScroll: true });
    expect(scrollShortcut(chord("y", { ctrlKey: true }), false, true)).toEqual({ key: "y", lineScroll: true });
    expect(scrollShortcut(chord("d", { ctrlKey: true }), false, false)).toEqual({ key: "d", lineScroll: false });
    expect(scrollShortcut(chord("d"), false, false)).toBeNull();
    expect(scrollShortcut(chord("e"), false, true)).toBeNull();
    expect(scrollShortcut(chord("d"), true, true)).toBeNull();
    expect(scrollShortcut(chord("d", { altKey: true }), false, true)).toBeNull();
    expect(scrollShortcut(chord("d", { metaKey: true }), false, true)).toBeNull();
    expect(scrollShortcut(chord("e", { ctrlKey: true, shiftKey: true }), false, true)).toBeNull();
    expect(scrollShortcut(chord("j"), false, true)).toBeNull();
  });

  it("derives the line from the computed style with a fallback on the font size", () => {
    expect(lineHeight("24px", "16px")).toBe(24);
    expect(lineHeight("normal", "20px")).toBe(32);
  });

  it("scrolls one line, or the preferred lines capped at half the visible content, in the key's direction", () => {
    expect(scrollDistance({ key: "e", lineScroll: true }, 24, 800, 10)).toBe(24);
    expect(scrollDistance({ key: "y", lineScroll: true }, 24, 800, 10)).toBe(-24);
    expect(scrollDistance({ key: "d", lineScroll: false }, 24, 800, 10)).toBe(240);
    expect(scrollDistance({ key: "u", lineScroll: false }, 24, 800, 100)).toBe(-400);
    expect(scrollDistance({ key: "d", lineScroll: false }, 24, 10, 100)).toBe(12);
  });
});

it("steps through the feed with j and k on an article", () => {
  expect(articleNavigationDirection("j")).toBe("next");
  expect(articleNavigationDirection("K")).toBe("previous");
  expect(articleNavigationDirection("Enter")).toBeNull();
  expect(articleNavigationDirection("x")).toBeNull();
});

describe("article navigation targets", () => {
  it("keeps adjacent article URLs for both keyboard and swipe directions", () => {
    const links = { previousUrl: "items/newer/", nextUrl: "items/older/" };
    expect(articleNavigationTarget("previous", links, base)).toBe("https://reader.test/nested/items/newer/");
    expect(articleNavigationTarget("next", links, base)).toBe("https://reader.test/nested/items/older/");
  });

  it("returns to the feed past either archive boundary", () => {
    expect(articleNavigationTarget("previous", { nextUrl: "items/older/" }, base)).toBe(base);
    expect(articleNavigationTarget("next", { previousUrl: "items/newer/" }, base)).toBe(base);
  });

  it("returns to the feed in both directions for a single article", () => {
    expect(articleNavigationTarget("previous", {}, base)).toBe(base);
    expect(articleNavigationTarget("next", { nextUrl: "" }, base)).toBe(base);
  });
});
