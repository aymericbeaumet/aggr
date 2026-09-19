/** Which feed items are new since the reader last looked, remembered for the browsing session. */

export function entryStateKey(name: string, base: string): string {
  return "aggr:" + name + ":" + encodeURIComponent(new URL(base).pathname);
}

export type AgeBand = "fresh" | "h1" | "h3" | "h24";

/** How old a published time is, in the bands the stylesheet colours. */
export function ageBand(iso: string, now: number = Date.now()): AgeBand {
  const age = Math.max(0, now - Date.parse(iso));
  if (age < 60 * 60 * 1000) return "fresh";
  if (age < 3 * 60 * 60 * 1000) return "h1";
  if (age < 24 * 60 * 60 * 1000) return "h3";
  return "h24";
}

export function uniqueEntries(entries: string[]): string[] {
  return Array.from(new Set(entries));
}

/** Absolute, deduplicated entry URLs; entries that do not parse are dropped. */
export function resolveEntries(entries: string[], base: string): string[] {
  return uniqueEntries(entries.map(function (entry) {
    try { return new URL(entry, base).href; } catch (error) { return null; }
  }).filter((entry): entry is string => entry !== null));
}

/** Entries above the previously seen head are new; without a head nothing is (a first visit highlights nothing). */
export function mergeNewEntries(previousHead: string | null, current: string[], pending: string[]): string[] {
  if (!previousHead || !current.length) return pending;
  const boundary = current.indexOf(previousHead);
  const additions = current.slice(0, boundary === -1 ? current.length : boundary);
  return uniqueEntries(pending.concat(additions));
}

/** Manifest entries that belong to this site; anything else, and anything that is not a string, is ignored. */
export function scopedEntries(entries: unknown[], base: string): string[] {
  return entries.filter((entry): entry is string => {
    if (typeof entry !== "string") return false;
    try { return new URL(entry, base).href.startsWith(base); } catch (error) { return false; }
  });
}

/** The pending entries the reader has not acknowledged yet. */
export function remainingEntries(pending: string[], acknowledged: ReadonlySet<string>): string[] {
  return pending.filter(function (entry) { return !acknowledged.has(entry); });
}
