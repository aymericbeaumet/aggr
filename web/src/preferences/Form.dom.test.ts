import { flushSync } from 'svelte';
import { afterEach, describe, expect, it } from 'vitest';
import type { PreferenceSchema } from '../generated/PreferenceSchema';
import { page } from '../state/page.svelte';
import { loadPreferences, preferences } from '../state/preferences.svelte';
import { installPreferences } from './form.svelte';
import { applyDataset } from './rules';
import { encodeState } from './transfer';

const schema: PreferenceSchema = {
  bootstrap: {
    theme: { initial: 'system', values: ['system', 'light', 'dark'], attribute: 'theme' },
    'single-key-shortcuts': { initial: true, values: [true, false] },
    'scroll-amount': { initial: 10, min: 1, max: 100 },
  },
  groups: [
    {
      id: 'appearance',
      title: 'Appearance',
      fields: [
        {
          key: 'theme',
          label: 'Theme',
          control: 'select',
          value: 'system',
          options: [
            { value: 'system', label: 'System' },
            { value: 'light', label: 'Light' },
            { value: 'dark', label: 'Dark' },
          ],
        },
      ],
    },
    {
      id: 'keyboard',
      title: 'Keyboard',
      fields: [
        { key: 'single-key-shortcuts', label: 'Single-key shortcuts', control: 'checkbox', value: true, options: [] },
        { key: 'scroll-amount', label: 'Scroll amount', control: 'number', value: 10, options: [], min: 1, max: 100, help: 'Lines per step.' },
      ],
    },
  ],
};

let stop: (() => void) | null = null;
let originalTheme: Element | null = null;
// Node's own `localStorage` global shadows jsdom's and is undefined without a file, so the
// store is a map with the two methods the state uses.
const stored = new Map<string, string>();
const storage = {
  getItem: (key: string) => stored.get(key) ?? null,
  setItem: (key: string, value: string) => void stored.set(key, value),
  clear: () => stored.clear(),
};

/** Minimal server-rendered controls; the browser suite checks the complete Rust template. */
const form = `<fieldset id="preferences-controls" disabled hidden>
  <section class="preferences-group"><h2>Appearance</h2>
    <select id="theme" data-preference="theme"><option value="system">System</option><option value="light">Light</option><option value="dark">Dark</option></select>
    <p data-template-marker>Custom template content</p>
  </section>
  <section class="preferences-group"><h2>Keyboard</h2>
    <input id="single-key-shortcuts" data-preference="single-key-shortcuts" type="checkbox">
    <input id="scroll-amount" data-preference="scroll-amount" type="number" min="1" max="100" required aria-describedby="scroll-amount-help">
    <span id="scroll-amount-help">Lines per step.</span><button id="show-shortcuts" type="button">Show keyboard shortcuts</button>
  </section>
  <section class="preferences-group preferences-import" id="preferences-import" hidden><ul id="preferences-import-summary"></ul>
    <button data-preferences-action="apply" type="button">Apply preferences</button><button data-preferences-action="cancel" type="button">Cancel</button>
  </section>
  <button id="share-state" data-preferences-action="share" type="button" hidden>Share</button>
  <button id="copy-state" data-preferences-action="copy" type="button">Copy link</button>
  <button data-preferences-action="reset" type="button">Reset to defaults</button><span id="preferences-status"></span>
  <input id="preferences-link" type="url" hidden readonly><input id="preferences-file" type="file" hidden>
</fieldset>`;

function open(stored: Record<string, string> = {}, hash = ''): void {
  storage.clear();
  for (const [key, value] of Object.entries(stored)) storage.setItem(`aggr:${key}`, value);
  delete document.documentElement.dataset.theme;
  history.replaceState(null, '', `/reader/preferences/${hash}`);
  document.head.innerHTML = `<script id="aggr-preferences" type="application/json">${JSON.stringify(schema.bootstrap)}</script>`;
  document.body.innerHTML = `<main id="content">${form}</main>`;
  page.root = new URL('/reader/', document.baseURI).href;
  preferences.load(document, storage);
  // What the bootstrap wrote before paint.
  applyDataset(preferences.schema, preferences.values ?? {}, document.documentElement);
  preferences.pending = null;
  preferences.status = '';
  originalTheme = document.getElementById('theme');
  stop = installPreferences(document.getElementById('content') as HTMLElement, schema);
  flushSync();
}

const status = () => document.getElementById('preferences-status')?.textContent;
const control = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const change = (element: HTMLElement) => element.dispatchEvent(new Event('change', { bubbles: true }));

afterEach(() => {
  stop?.();
  stop = null;
});

describe('preferences form enhancement', () => {
  it('preserves rendered controls and custom content while showing stored values', () => {
    open({ theme: 'dark', 'scroll-amount': '3' });
    expect(control('theme')).toBe(originalTheme);
    expect(document.querySelector('[data-template-marker]')?.textContent).toBe('Custom template content');
    expect(control<HTMLFieldSetElement>('preferences-controls').disabled).toBe(false);
    expect(control<HTMLFieldSetElement>('preferences-controls').hidden).toBe(false);
    expect([...document.querySelectorAll('[data-preference]')].map((element) => element.id)).toEqual([
      'theme',
      'single-key-shortcuts',
      'scroll-amount',
    ]);
    expect([...document.querySelectorAll('.preferences-group:not(.preferences-import) > h2')].map((h) => h.textContent)).toEqual([
      'Appearance',
      'Keyboard',
    ]);
    expect(control<HTMLSelectElement>('theme').value).toBe('dark');
    expect(control<HTMLInputElement>('single-key-shortcuts').checked).toBe(true);
    expect(control<HTMLInputElement>('scroll-amount').value).toBe('3');
    expect(control<HTMLInputElement>('scroll-amount').getAttribute('aria-describedby')).toBe('scroll-amount-help');
    expect(document.getElementById('scroll-amount-help')?.textContent).toBe('Lines per step.');
    expect(document.getElementById('show-shortcuts')?.tagName).toBe('BUTTON');
    expect(document.getElementById('preferences-controls')).toBeInstanceOf(HTMLFieldSetElement);
    expect(control<HTMLElement>('preferences-import').hidden).toBe(true);
    expect(control<HTMLInputElement>('preferences-link').hidden).toBe(true);
  });

  it('applies a changed control to the document and to storage at once', () => {
    open();
    const theme = control<HTMLSelectElement>('theme');
    theme.value = 'dark';
    change(theme);
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(storage.getItem('aggr:theme')).toBe('dark');
    expect(preferences.values?.theme).toBe('dark');
    flushSync();
    expect(status()).toBe('Saved in this browser.');
    const shortcuts = control<HTMLInputElement>('single-key-shortcuts');
    shortcuts.checked = false;
    change(shortcuts);
    expect(storage.getItem('aggr:single-key-shortcuts')).toBe('false');
    expect(preferences.singleKeyShortcuts).toBe(false);
  });

  it('leaves an out-of-range number out of storage', () => {
    open();
    const amount = control<HTMLInputElement>('scroll-amount');
    amount.value = '500';
    change(amount);
    expect(storage.getItem('aggr:scroll-amount')).toBeNull();
    expect(preferences.scrollAmount).toBe(10);
    amount.value = '20';
    change(amount);
    expect(storage.getItem('aggr:scroll-amount')).toBe('20');
    expect(preferences.scrollAmount).toBe(20);
  });

  it('reviews an import before applying it, and resets to the defaults', () => {
    open();
    preferences.review({ theme: 'dark', 'single-key-shortcuts': false });
    flushSync();
    const review = control<HTMLElement>('preferences-import');
    expect(review.hidden).toBe(false);
    expect([...document.querySelectorAll('#preferences-import-summary li')].map((li) => li.textContent)).toEqual([
      'Theme: Dark',
      'Single-key shortcuts: Off',
    ]);
    expect(status()).toBe('Review the imported settings before applying them.');
    expect(document.documentElement.dataset.theme).toBe('system');
    expect(storage.getItem('aggr:theme')).toBeNull();
    document.querySelector<HTMLButtonElement>('[data-preferences-action="apply"]')?.click();
    flushSync();
    expect(review.hidden).toBe(true);
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(storage.getItem('aggr:theme')).toBe('dark');
    expect(control<HTMLSelectElement>('theme').value).toBe('dark');
    expect(control<HTMLInputElement>('single-key-shortcuts').checked).toBe(false);
    document.querySelector<HTMLButtonElement>('[data-preferences-action="reset"]')?.click();
    flushSync();
    expect(document.documentElement.dataset.theme).toBe('system');
    expect(storage.getItem('aggr:theme')).toBe('system');
    expect(control<HTMLSelectElement>('theme').value).toBe('system');
    expect(status()).toBe('Default preferences restored.');
  });

  it('cancels a review without changing anything', () => {
    open({ theme: 'light' });
    preferences.review({ theme: 'dark' });
    flushSync();
    document.querySelector<HTMLButtonElement>('[data-preferences-action="cancel"]')?.click();
    flushSync();
    expect(control<HTMLElement>('preferences-import').hidden).toBe(true);
    expect(status()).toBe('Import cancelled. Your preferences were not changed.');
    expect(storage.getItem('aggr:theme')).toBe('light');
  });

  it('takes a shared link out of the address and into the review', () => {
    open({}, `#aggr-state=${encodeState({ theme: 'dark' })}`);
    expect(location.hash).toBe('');
    expect(control<HTMLElement>('preferences-import').hidden).toBe(false);
    expect(document.querySelector('#preferences-import-summary li')?.textContent).toBe('Theme: Dark');
    expect(document.documentElement.dataset.theme).toBe('system');
  });

  it('refuses an invalid link and ignores a query string', () => {
    open({ theme: 'light' }, '#aggr-state=not_valid!');
    expect(control<HTMLElement>('preferences-import').hidden).toBe(true);
    expect(status()).toContain('invalid or unsupported');
    expect(document.documentElement.dataset.theme).toBe('light');
    stop?.();
    stop = null;
    open({ theme: 'light' }, `?aggr-state=${encodeState({ theme: 'dark' })}`);
    expect(control<HTMLElement>('preferences-import').hidden).toBe(true);
    expect(status()).toBe('');
    expect(location.search).toContain('aggr-state=');
  });

  it('offers the link to copy by hand where the clipboard is out of reach', async () => {
    open({ theme: 'dark' });
    control<HTMLButtonElement>('copy-state').click();
    await Promise.resolve();
    flushSync();
    const field = control<HTMLInputElement>('preferences-link');
    expect(field.hidden).toBe(false);
    const url = new URL(field.value);
    expect(url.pathname).toBe('/reader/preferences/');
    expect(url.hash.startsWith('#aggr-state=')).toBe(true);
    expect(url.searchParams.has('aggr-state')).toBe(false);
    expect(status()).toBe('Copy the selected link. Clipboard access is unavailable.');
    expect(control<HTMLButtonElement>('share-state').hidden).toBe(true);
  });

  it('follows a change made in another tab', () => {
    open();
    loadPreferences(document, storage, window);
    storage.setItem('aggr:theme', 'dark');
    window.dispatchEvent(new StorageEvent('storage', { key: 'aggr:theme' }));
    expect(preferences.values?.theme).toBe('dark');
    expect(document.documentElement.dataset.theme).toBe('dark');
    flushSync();
    expect(control<HTMLSelectElement>('theme').value).toBe('dark');
  });

  it('releases listeners and reactive updates when the page leaves', () => {
    open();
    stop?.();
    stop = null;
    const theme = control<HTMLSelectElement>('theme');
    theme.value = 'dark';
    change(theme);
    expect(storage.getItem('aggr:theme')).toBeNull();
    preferences.apply({ theme: 'light' });
    flushSync();
    expect(theme.value).toBe('dark');
  });
});
