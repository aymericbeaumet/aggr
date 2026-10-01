<script lang="ts">
  // The preferences form, loaded with the page: every control comes from the typed table in
  // src/config/preferences.rs, through the page model's schema, and shows the loaded state.
  // The markup keeps the ids, classes and `data-preference` attributes `style.css` and the
  // browser suite address.
  import { flushSync, onMount } from 'svelte';
  import type { PreferenceField } from '../generated/PreferenceField';
  import type { PreferenceSchema } from '../generated/PreferenceSchema';
  import type { PreferenceValue } from './rules';
  import { openShortcutHelp } from '../shortcuts/help';
  import { page as shown } from '../state/page.svelte';
  import { messages, preferences } from '../state/preferences.svelte';
  import { copyLink, importFile, importFragment, saveFile, share } from './actions';

  let { schema }: { schema: PreferenceSchema } = $props();

  const values = $derived(preferences.values ?? {});
  const fields = $derived(new Map(schema.groups.flatMap((group) => group.fields.map((field) => [field.key, field] as const))));
  const canShare = typeof navigator !== 'undefined' && typeof navigator.share === 'function';
  const root = () => shown.root || document.baseURI;

  let link = $state('');
  let linkField: HTMLInputElement | undefined = $state();
  let fileField: HTMLInputElement | undefined = $state();

  /** The control's current text: the loaded value, else what the build rendered. */
  const text = (field: PreferenceField): string => String(values[field.key] ?? field.value);

  /** A changed control: validated by the browser first, then applied and persisted at once. */
  function change(event: Event) {
    const control = event.currentTarget;
    if (!(control instanceof HTMLInputElement || control instanceof HTMLSelectElement)) return;
    const key = control.dataset.preference;
    if (!key) return;
    if (!control.checkValidity()) {
      control.reportValidity();
      return;
    }
    const value =
      control instanceof HTMLInputElement && control.type === 'checkbox'
        ? control.checked
        : control.type === 'number'
          ? Number(control.value)
          : control.value;
    const saved = preferences.apply({ [key]: value });
    preferences.status = saved ? messages.saved : messages.sessionOnly;
    // A value the rules refused stays out of the control too.
    if (control instanceof HTMLSelectElement || control.type === 'number') control.value = String(values[key] ?? '');
    else if (control.type === 'checkbox') control.checked = Boolean(values[key]);
  }

  /** How an imported value reads in the review: its option label, On/Off, or the number. */
  function summary(key: string, value: PreferenceValue): string {
    const field = fields.get(key);
    const option = field?.options.find((entry) => entry.value === String(value))?.label;
    const display = typeof value === 'boolean' ? (value ? 'On' : 'Off') : (option ?? String(value));
    return `${field?.label ?? key}: ${display}`;
  }

  function offerLink(url: string) {
    link = url;
    flushSync();
    linkField?.focus();
    linkField?.select();
  }

  function pickFile() {
    const file = fileField?.files?.[0];
    if (fileField) fileField.value = '';
    if (file) void importFile(file);
  }

  /** Where invoker commands are unsupported, the button opens the dialog by hand. */
  function showShortcuts() {
    if (!('command' in HTMLButtonElement.prototype)) openShortcutHelp();
  }

  onMount(() => importFragment());
</script>

{#snippet help(id: string, label: string, explanation: string)}<details class="preference-help"><summary aria-label={label} title={explanation}>?</summary><span {id} class="preference-help-text">{explanation}</span></details>{/snippet}

<fieldset id="preferences-controls" class="preferences-controls">
{#each schema.groups as group (group.id)}
<section class="preferences-group" aria-labelledby="{group.id}-heading">
  <h2 id="{group.id}-heading">{group.title}</h2>
  {#each group.fields as field (field.key)}
  {#if field.control === 'select'}
  <label class="setting" for={field.key}>
    <strong>{field.label}</strong>
    <select id={field.key} data-preference={field.key} value={text(field)} onchange={change}>
      {#each field.options as option (option.value)}<option value={option.value}>{option.label}</option>{/each}
    </select>
  </label>
  {:else if field.control === 'checkbox'}
  <label class="setting" for={field.key}>
    <strong>{field.label}</strong>
    <input id={field.key} data-preference={field.key} type="checkbox" checked={Boolean(values[field.key] ?? field.value)} onchange={change}>
  </label>
  {:else}
  <div class="setting">
    <div class="setting-label"><label for={field.key}>{field.label}</label>{#if field.help}{@render help(`${field.key}-help`, `About ${field.label.toLowerCase()}`, field.help)}{/if}</div>
    <input id={field.key} data-preference={field.key} type="number" required min={field.min} max={field.max} step="1" aria-describedby={field.help ? `${field.key}-help` : undefined} value={text(field)} onchange={change}>
  </div>
  {/if}
  {/each}
  {#if group.id === 'offline'}<p id="offline-download-status" class="muted" role="status" aria-live="polite">Checking offline availability…</p>{/if}
  {#if group.id === 'keyboard'}
  <p class="setting"><button class="text-button" type="button" id="show-shortcuts" command="show-modal" commandfor="shortcut-help" aria-haspopup="dialog" onclick={showShortcuts}>Show keyboard shortcuts</button></p>
  {/if}
</section>
{/each}
<section id="preferences-import" class="preferences-group preferences-import" aria-labelledby="preferences-import-heading" hidden={!preferences.pending}>
  <h2 id="preferences-import-heading">Review imported preferences</h2>
  <ul id="preferences-import-summary">{#each Object.entries(preferences.pending ?? {}) as [key, value] (key)}<li>{summary(key, value)}</li>{/each}</ul>
  <div class="preference-actions"><button data-preferences-action="apply" type="button" onclick={() => preferences.confirm()}>Apply preferences</button><button data-preferences-action="cancel" type="button" onclick={() => preferences.cancel()}>Cancel</button></div>
</section>
<section class="preferences-group preferences-transfer" aria-labelledby="preferences-transfer-heading">
  <div class="preferences-group-heading"><h2 id="preferences-transfer-heading">Transfer preferences</h2>{@render help('preferences-transfer-help', 'About transferring preferences', 'Use the same settings on another device. Links and files contain only the options above, never reading history.')}</div>
  <div class="preference-actions">
    <button id="share-state" data-preferences-action="share" type="button" hidden={!canShare} onclick={() => void share(root())}>Share</button>
    <button id="copy-state" data-preferences-action="copy" type="button" onclick={() => void copyLink(root(), offerLink)}>Copy link</button>
    <button data-preferences-action="save" type="button" onclick={() => saveFile()}>Save file</button>
    <button data-preferences-action="import" type="button" onclick={() => fileField?.click()}>Import file</button>
  </div>
  <div class="preference-actions preferences-reset"><button class="text-button" data-preferences-action="reset" type="button" onclick={() => preferences.reset()}>Reset to defaults</button><span id="preferences-status" class="muted" role="status" aria-live="polite">{preferences.status}</span></div>
  <input id="preferences-link" type="url" aria-label="Preferences link" readonly hidden={!link} value={link} bind:this={linkField}>
  <input id="preferences-file" type="file" accept="application/json,.json" hidden bind:this={fileField} onchange={pickFile}>
</section>
</fieldset>
