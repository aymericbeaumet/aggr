/**
 * The contracts the reader's scripts share with the MiniJinja templates.
 *
 * Nothing imports this at runtime: it exists so editors type-check
 * `themes/default/static/*.js` through `jsconfig.json`, with no toolchain installed.
 */

/** A discussion network, from `[[networks]]` in aggr.toml. */
export interface DiscussionNetwork {
  name: string;
  /** Upper-case single key that opens this network for the current article. */
  shortcut?: string;
  /** Search template with `{url}` and `{title}` placeholders. */
  url: string;
}

/** Hashed URLs of the scripts loaded on demand; `url_for` resolves them at render time. */
export interface AssetMap {
  search: string;
  media: string;
}

/** `window.AGGR`, emitted by `base.html`. */
export interface AppContext {
  /** Site root, relative to the page. */
  base: string;
  /** Page kind: `river`, `item`, `browse`, `preferences`, … */
  kind: string;
  /** Whether a manifest and service worker were built. */
  pwa: boolean;
  discussions: DiscussionNetwork[];
  /** The first nine feed entries, for the `g 1` … `g 9` shortcuts. */
  entries: string[];
  assets: AssetMap;
  /** The content fingerprint this page was built from, for `updates.json` polling. */
  content?: string;
  /** Where the search index lives, when one was built. */
  search?: { base: string; docs: number };
}

export type PreferenceValue = string | number | boolean;

/** One setting's validation rule, generated from `src/config/preferences.rs`. */
export interface PreferenceRule {
  initial: PreferenceValue;
  values?: PreferenceValue[];
  min?: number;
  max?: number;
  /** `documentElement.dataset` key this setting drives. */
  attribute?: string;
}

/** `window.AGGRPreferences`, the pre-paint bootstrap in `base.html`. */
export interface Preferences {
  schema: Record<string, PreferenceRule>;
  values: Record<string, PreferenceValue>;
  read(): Record<string, PreferenceValue>;
  validate(payload: unknown): Record<string, PreferenceValue>;
  valid(key: string, value: unknown): boolean;
  /** Mirror the values onto `documentElement.dataset`. */
  apply(values: Record<string, PreferenceValue>): void;
}

/** `window.AGGRDates`, the pre-paint date bootstrap in `_dates.html`. */
export interface SharedDates {
  text(timestamp: number, format: string): string | null;
}

declare global {
  interface Window {
    AGGR?: AppContext;
    AGGRPreferences?: Preferences;
    AGGRDates?: SharedDates;
  }
}
