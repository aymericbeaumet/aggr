/**
 * The contracts the reader's scripts share with the MiniJinja templates.
 *
 * Nothing imports this at runtime: editors and the developer-only compiler check
 * `themes/default/static/*.js` through `jsconfig.json`.
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
  reader: string;
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
  /** The application fingerprint, separate from content updates. */
  app?: string;
  /** Where the search index lives, when one was built. */
  search?: { base: string; docs: number };
}

export type PreferenceValue = string | number | boolean;

/**
 * One setting's validation rule, generated from `src/config/preferences.rs`: a select or a
 * checkbox lists its values, a number carries its bounds instead.
 */
export type PreferenceRule = ChoiceRule | RangeRule;

export interface ChoiceRule {
  initial: PreferenceValue;
  values: PreferenceValue[];
  min?: undefined;
  max?: undefined;
  /** `documentElement.dataset` key this setting drives. */
  attribute?: string;
}

export interface RangeRule {
  initial: number;
  values?: undefined;
  min: number;
  max: number;
  /** `documentElement.dataset` key this setting drives. */
  attribute?: string;
}

/** `window.AGGRPreferences`, the pre-paint bootstrap in `base.html`. */
export interface Preferences {
  schema: Record<string, PreferenceRule>;
  values: Record<string, PreferenceValue>;
  read(): Record<string, PreferenceValue>;
  validate(payload: unknown): Record<string, PreferenceValue>;
  valid(key: string, value: unknown): value is PreferenceValue;
  /** Mirror the values onto `documentElement.dataset`. */
  apply(values: Record<string, PreferenceValue>): void;
}

/** `window.AGGRDates`, the pre-paint date bootstrap in `_dates.html`. */
export interface SharedDates {
  text(timestamp: number, format: string): string | null;
}

export interface OfflineStatus {
  requested: number;
  total: number;
  saved: Array<{ url: string; title: string }>;
  failed: number;
  downloading: boolean;
  search?: {
    phase: string;
    activeVersion: string | null;
    targetVersion: string | null;
    base: string | null;
    downloadedFiles: number;
    totalFiles: number;
    downloadedBytes: number;
    totalBytes: number;
    error: string | null;
  };
}

declare global {
  interface Navigator {
    connection?: { saveData?: boolean; effectiveType?: string };
    userAgentData?: { platform: string };
  }

  interface Document {
    prerendering?: boolean;
  }

  interface Window {
    AGGR?: AppContext;
    AGGRPreferences?: Preferences;
    AGGRDates?: SharedDates;
    AGGROffline?: OfflineStatus;
  }
}
