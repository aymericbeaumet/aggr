import type { PreferenceSchema } from '../generated/PreferenceSchema';
import { openShortcutHelp } from '../shortcuts/help';
import { page } from '../state/page.svelte';
import { messages, preferences } from '../state/preferences.svelte';
import { copyLink, importFile, importFragment, saveFile, share } from './actions';
import type { PreferenceValue } from './rules';

/** Bind browser-local settings to the controls rendered by Rust. */
export function installPreferences(root: HTMLElement, schema: PreferenceSchema): () => void {
  const controls = root.querySelector<HTMLFieldSetElement>('#preferences-controls');
  if (!controls) return () => {};
  const fields = new Map(schema.groups.flatMap((group) => group.fields.map((field) => [field.key, field] as const)));
  const inputs = [...controls.querySelectorAll<HTMLInputElement | HTMLSelectElement>('[data-preference]')];
  const linkField = controls.querySelector<HTMLInputElement>('#preferences-link');
  const fileField = controls.querySelector<HTMLInputElement>('#preferences-file');
  const review = controls.querySelector<HTMLElement>('#preferences-import');
  const summaryList = controls.querySelector<HTMLElement>('#preferences-import-summary');
  const status = controls.querySelector<HTMLElement>('#preferences-status');
  const shareButton = controls.querySelector<HTMLButtonElement>('#share-state');
  if (shareButton) shareButton.hidden = typeof navigator.share !== 'function';
  const controller = new AbortController();
  const { signal } = controller;

  const summary = (key: string, value: PreferenceValue): string => {
    const field = fields.get(key);
    const option = field?.options.find((entry) => entry.value === String(value))?.label;
    const display = typeof value === 'boolean' ? (value ? 'On' : 'Off') : (option ?? String(value));
    return `${field?.label ?? key}: ${display}`;
  };
  const sync = () => {
    for (const control of inputs) {
      const key = control.dataset.preference ?? '';
      const value = preferences.values?.[key] ?? fields.get(key)?.value;
      if (control instanceof HTMLInputElement && control.type === 'checkbox') control.checked = Boolean(value);
      else if (control.value !== String(value ?? '')) control.value = String(value ?? '');
    }
    if (review) review.hidden = !preferences.pending;
    if (summaryList) summaryList.replaceChildren(...Object.entries(preferences.pending ?? {}).map(([key, value]) => {
      const item = document.createElement('li');
      item.textContent = summary(key, value);
      return item;
    }));
    if (status) status.textContent = preferences.status;
  };

  controls.addEventListener('change', (event) => {
    const control = event.target;
    if (!(control instanceof HTMLInputElement || control instanceof HTMLSelectElement)) return;
    if (control === fileField) {
      const file = fileField.files?.[0];
      fileField.value = '';
      if (file) void importFile(file);
      return;
    }
    const key = control.dataset.preference;
    if (!key || !fields.has(key)) return;
    if (!control.checkValidity()) {
      control.reportValidity();
      return;
    }
    const value = control instanceof HTMLInputElement && control.type === 'checkbox'
      ? control.checked : control.type === 'number' ? Number(control.value) : control.value;
    preferences.status = preferences.apply({ [key]: value }) ? messages.saved : messages.sessionOnly;
    sync();
  }, { signal });

  const offerLink = (url: string) => {
    if (!linkField || signal.aborted) return;
    linkField.value = url;
    linkField.hidden = false;
    linkField.focus();
    linkField.select();
  };
  controls.addEventListener('click', (event) => {
    const button = event.target instanceof Element ? event.target.closest<HTMLButtonElement>('button') : null;
    if (!button || !controls.contains(button)) return;
    const base = page.root || document.baseURI;
    switch (button.dataset.preferencesAction) {
      case 'apply': preferences.confirm(); break;
      case 'cancel': preferences.cancel(); break;
      case 'share': void share(base); break;
      case 'copy': void copyLink(base, offerLink); break;
      case 'save': saveFile(); break;
      case 'import': fileField?.click(); break;
      case 'reset': preferences.reset(); break;
    }
    if (button.id === 'show-shortcuts' && !('command' in HTMLButtonElement.prototype)) openShortcutHelp();
  }, { signal });

  importFragment();
  sync();
  controls.disabled = false;
  controls.hidden = false;
  const stop = $effect.root(() => { $effect(sync); });
  return () => { controller.abort(); stop(); };
}
