/**
 * Keyboard shortcut decisions that need no page: which route a `g` chord opens, where an external
 * shortcut goes, and how far a scroll key travels.
 */

export type GotoTarget = { type: "first" } | { type: "navigate"; url: string };

/** The second key of a `g` chord: `gg` goes to the top, letters open routes, digits open the numbered entries. */
export function gotoRoute(key: string, entries: readonly string[], base: string): GotoTarget | null {
  if (key === "g") return { type: "first" };
  const routes: Record<string, string> = { f: "", i: "", l: "browse/", p: "preferences/" };
  if (Object.prototype.hasOwnProperty.call(routes, key)) return { type: "navigate", url: new URL(routes[key], base).href };
  if (/^[1-9]$/.test(key)) {
    const entry = entries[Number(key) - 1];
    if (entry) return { type: "navigate", url: new URL(entry, base).href };
  }
  return null;
}

export interface DiscussionNetwork {
  name: string;
  shortcut?: string;
  url: string;
}

export interface ExternalArticle {
  /** The `.u-bookmark-of` link, when the article has one. */
  original: string | null;
  /** The article's canonical link (`data-link`). */
  link: string;
  title: string;
  /** Discussion links already rendered in the article, by network name. */
  discussions: ReadonlyArray<{ name: string; href: string }>;
}

/** Only an upper-case single key is an external shortcut. */
export function externalShortcutKey(key: string): boolean {
  return key.length === 1 && key === key.toUpperCase();
}

/** Where `key` sends the reader: `O` opens the original; a network's shortcut opens its rendered discussion link, or its search template. */
export function externalShortcutTarget(key: string, article: ExternalArticle, networks: readonly DiscussionNetwork[]): string | null {
  if (!externalShortcutKey(key)) return null;
  if (key === "O") return article.original;
  const network = networks.find(function (candidate) { return candidate.shortcut === key; });
  if (!network) return null;
  const found = article.discussions.find(function (link) { return link.name === network.name; });
  return found ? found.href : network.url
    .split("{url}").join(encodeURIComponent(article.link))
    .split("{title}").join(encodeURIComponent(article.title));
}

export interface KeyChord {
  key: string;
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
}

export interface ScrollKey {
  key: "d" | "u" | "e" | "y";
  lineScroll: boolean;
}

/** `d`/`u` scroll by the preferred amount and `ctrl-e`/`ctrl-y` by one line; editing fields and other chords are left alone. */
export function scrollShortcut(chord: KeyChord, editing: boolean, singleKeyShortcuts: boolean): ScrollKey | null {
  if (chord.altKey || chord.metaKey || editing) return null;
  if (chord.ctrlKey && chord.shiftKey) return null;
  if (!chord.ctrlKey && !singleKeyShortcuts) return null;
  const key = chord.key.toLowerCase();
  const lineScroll = chord.ctrlKey && (key === "e" || key === "y");
  if (!lineScroll && key !== "d" && key !== "u") return null;
  return { key: key as ScrollKey["key"], lineScroll };
}

/** The line the scroll keys step by, from the content's computed style. */
export function lineHeight(lineHeightValue: string, fontSize: string): number {
  return parseFloat(lineHeightValue) || parseFloat(fontSize) * 1.6;
}

/** Signed scroll distance: one line, or the preferred number of lines capped at half the visible content. */
export function scrollDistance(scroll: ScrollKey, line: number, visibleHeight: number, scrollAmount: number): number {
  const available = Math.max(line, visibleHeight);
  const distance = scroll.lineScroll ? line : Math.min(line * scrollAmount, available * 0.5);
  return scroll.key === "d" || scroll.key === "e" ? distance : -distance;
}

/** `j`/`k` on an article page step through the feed. */
export function articleNavigationDirection(key: string): "previous" | "next" | null {
  const normalized = key.length === 1 ? key.toLowerCase() : key;
  if (normalized === "k") return "previous";
  if (normalized === "j") return "next";
  return null;
}

/**
 * The absolute address of the neighbouring article: its site-relative path resolved against the
 * site root (never the current page). Leaving either end of the article sequence returns to the feed.
 */
export function articleNavigationTarget(
  direction: "previous" | "next",
  links: { previousUrl?: string | null; nextUrl?: string | null },
  base: string,
): string {
  const target = links[direction === "next" ? "nextUrl" : "previousUrl"];
  if (!target) return base;
  try { return new URL(target, base).href; } catch { return base; }
}
