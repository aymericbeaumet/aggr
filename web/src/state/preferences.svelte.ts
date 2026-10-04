import { readModel } from '../model/page';
import {
  applyDataset,
  defaults,
  parseSchema,
  positiveInteger,
  readPreferences,
  storageKey,
  valid,
  type PreferenceValue,
  type Schema,
} from '../preferences/rules';

export type Values = Record<string, PreferenceValue>;

/** What the reader is told on `/preferences/`, in the words the browser suite looks for. */
export const messages = {
  saved: 'Saved in this browser.',
  sessionOnly: 'Applied for this session; browser storage is unavailable.',
  review: 'Review the imported settings before applying them.',
  cancelled: 'Import cancelled. Your preferences were not changed.',
  reset: 'Default preferences restored.',
  resetSessionOnly: 'Defaults applied for this session; browser storage is unavailable.',
  invalidLink: 'This preferences link is invalid or unsupported. Nothing was changed.',
  invalidFile:
    'This file is invalid or unsupported. Use an aggr preferences JSON file (up to 16 KB). Nothing was changed.',
  copied: 'Preferences link copied.',
  offered: 'Copy the selected link. Clipboard access is unavailable.',
  unshareable: 'Sharing is unavailable. Use Copy link or Save file.',
  savedFile: 'Preferences file saved.',
} as const;

type Storage = Pick<globalThis.Storage, 'getItem' | 'setItem'>;

/**
 * The reader's settings, as the bootstrap applied them before paint: read from `localStorage`
 * and validated against the `#aggr-preferences` rules. Until loaded (server renders, documents
 * without the app) every setting is unknown and the components show what the build wrote.
 * Changes go to the document, to storage and to the other tabs from here; the form on
 * `/preferences/` is a view of this state.
 */
class Preferences {
  schema: Schema = {};
  values = $state.raw<Values | null>(null);
  /** Settings from a link or a file, shown for review and applied only on confirmation. */
  pending = $state.raw<Values | null>(null);
  /** What `#preferences-status` announces. */
  status = $state('');
  private doc: Document | null = null;
  private storage: Storage | null = null;

  /** The `date-format` style, or undefined until the preferences are loaded. */
  get dateFormat(): string | undefined {
    const value = this.values?.['date-format'];
    return typeof value === 'string' ? value : undefined;
  }

  get feedPageSize(): number {
    return positiveInteger(this.values?.['feed-page-size'], 25);
  }

  get singleKeyShortcuts(): boolean {
    return this.values?.['single-key-shortcuts'] !== false;
  }

  get scrollAmount(): number {
    return positiveInteger(this.values?.['scroll-amount'], 10);
  }

  get motion(): string {
    return String(this.values?.motion ?? 'auto');
  }

  /** Offline downloads require an explicit site default or saved preference. */
  get offlineItems(): number {
    const value = this.values?.['offline-items'];
    return typeof value === 'number' && Number.isInteger(value) && value >= 0 ? value : 0;
  }

  /** Read the rules from the page and the stored values from `storage`. */
  load(doc: Document, storage: Storage | null): void {
    this.doc = doc;
    this.storage = storage;
    this.schema = parseSchema(doc.getElementById('aggr-preferences')?.textContent);
    this.values = this.read();
  }

  /** Another tab changed a setting: show what is stored now, without writing it back. */
  refresh(): void {
    this.adopt(this.read());
  }

  /**
   * Validate, show and persist `changes`; an invalid value is ignored, never stored. Returns
   * whether everything was written, so the reader can be told when it only holds for the session.
   */
  apply(changes: Record<string, unknown>, persist = true): boolean {
    const next: Values = { ...(this.values ?? defaults(this.schema)) };
    let saved = true;
    for (const [key, value] of Object.entries(changes)) {
      const rule = Object.hasOwn(this.schema, key) ? this.schema[key] : undefined;
      if (!rule || !valid(rule, value)) continue;
      next[key] = value;
      if (persist && !this.write(key, value)) saved = false;
    }
    this.adopt(next);
    return saved;
  }

  /** Every setting back to the value the build declared. */
  reset(): void {
    const saved = this.apply(defaults(this.schema));
    this.pending = null;
    this.status = saved ? messages.reset : messages.resetSessionOnly;
  }

  /** Offer imported settings for review; nothing changes until `confirm`. */
  review(values: Values): void {
    this.pending = values;
    this.status = messages.review;
  }

  confirm(): void {
    if (!this.pending) return;
    const saved = this.apply(this.pending);
    this.pending = null;
    this.status = saved ? messages.saved : messages.sessionOnly;
  }

  /** Drop the pending import, saying why. */
  cancel(message: string = messages.cancelled): void {
    this.pending = null;
    this.status = message;
  }

  private read(): Values {
    const storage = this.storage;
    return readPreferences(this.schema, (key) => (storage ? storage.getItem(storageKey(key)) : null));
  }

  private write(key: string, value: PreferenceValue): boolean {
    try {
      if (!this.storage) return false;
      this.storage.setItem(storageKey(key), String(value));
      return true;
    } catch {
      return false; // quota, private browsing
    }
  }

  /** Show `values`: in the state, on `<html>` for the stylesheet, and in the browser chrome. */
  private adopt(values: Values): void {
    this.values = values;
    if (!this.doc) return;
    applyDataset(this.schema, values, this.doc.documentElement);
    refreshThemeColor(this.doc);
  }
}

export const preferences = new Preferences();

/** The browser chrome follows the header: its colour is whatever the theme gave `--nav-bg`. */
export function refreshThemeColor(doc: Document): void {
  const meta = doc.getElementById('theme-color');
  const view = doc.defaultView;
  if (!meta || !view) return;
  const color = view.getComputedStyle(doc.documentElement).getPropertyValue('--nav-bg').trim();
  if (color) meta.setAttribute('content', color);
}

/** `localStorage`, or null where the browser refuses it (cookies off, some private modes). */
function browserStorage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/**
 * A preferences link opened on another page of the site: the review belongs to `/preferences/`,
 * so go there with the payload still in the fragment. Returns whether the page is being left.
 */
export function redirectSharedState(doc: Document, win: Window): boolean {
  const hash = win.location.hash;
  if (!/^#(?:.*&)?aggr-state=/.test(hash) || doc.body?.dataset.kind === 'preferences') return false;
  const model = readModel(doc.getElementById('aggr-page')?.textContent);
  if (!model) return false;
  const destination = new URL('preferences/', new URL(model.base, doc.baseURI));
  destination.hash = hash;
  win.location.replace(destination.href);
  return true;
}

/** Load once, and again whenever another tab changes a setting. */
export function loadPreferences(doc: Document = document, storage: Storage | null = browserStorage(), win: Window = window): void {
  preferences.load(doc, storage);
  refreshThemeColor(doc);
  win.addEventListener('storage', (event) => {
    if (event.key === null || Object.keys(preferences.schema).some((key) => event.key === storageKey(key))) {
      preferences.refresh();
    }
  });
  redirectSharedState(doc, win);
}
