<script lang="ts">
  import { onMount, untrack } from "svelte";
  import type { PreferenceValue } from "../contracts";
  import type { PreferencesService } from "./service";
  import type { PreferenceActions } from "./actions";
  import { fields, preferenceDescription } from "./presentation";
  let { service, actions, onShortcuts }: { service: PreferencesService; actions: PreferenceActions; onShortcuts: () => void } = $props();
  let state = $state(untrack(() => service.getSnapshot()));
  onMount(() => service.subscribe(next => { state = next; }));
  function change(event: Event & { currentTarget: HTMLInputElement | HTMLSelectElement }) {
    const control = event.currentTarget;
    if (!control.checkValidity()) { control.reportValidity(); return; }
    const value = control instanceof HTMLInputElement && control.type === "checkbox" ? control.checked : control.type === "number" ? Number(control.value) : control.value;
    service.apply({ [control.dataset.preference || ""]: value });
  }
  function fileChange(event: Event & { currentTarget: HTMLInputElement }) {
    const file = event.currentTarget.files?.[0];
    event.currentTarget.value = "";
    if (file) void service.importFile(file);
  }
  function summary(key: string, value: PreferenceValue) {
    return preferenceDescription(key, value);
  }
</script>

{#snippet help(id: string, label: string, explanation: string)}
<details class="preference-help"><summary aria-label={label} title={explanation}>?</summary><span {id} class="preference-help-text">{explanation}</span></details>
{/snippet}
{#snippet selectSetting(key: string, id: string = key)}
{@const field = fields[key]}
<label class="setting" for={id}><strong>{field.label}</strong><select {id} data-preference={key} value={String(state.values[key])} onchange={change}>{#each field.options as option}<option value={option.value}>{option.label}</option>{/each}</select></label>
{/snippet}

    
    <section class="preferences-group" aria-labelledby="appearance-heading">
      <h2 id="appearance-heading">Appearance</h2>
      {@render selectSetting("theme", "theme-mode")}
      {@render selectSetting("motion")}
    </section>
    <section class="preferences-group" aria-labelledby="reading-heading">
      <h2 id="reading-heading">Reading</h2>
      {@render selectSetting("font-family")}
      {@render selectSetting("text-size")}
      {@render selectSetting("reading-width")}
      {@render selectSetting("line-spacing")}
      {@render selectSetting("paragraph-spacing")}
      <label class="setting" for="paragraph-indent"><strong>{fields["paragraph-indent"].label}</strong><input id="paragraph-indent" data-preference="paragraph-indent" type="checkbox" checked={Boolean(state.values["paragraph-indent"])} onchange={change}></label>
      {@render selectSetting("text-align")}
      {@render selectSetting("letter-spacing")}
      {@render selectSetting("word-spacing")}
    </section>
    <section class="preferences-group" aria-labelledby="feed-heading">
      <h2 id="feed-heading">Feed</h2>
      {@render selectSetting("density")}
      {@render selectSetting("thumbnails")}
      {@render selectSetting("feed-page-size")}
      {@render selectSetting("date-format")}
    </section>
    <section class="preferences-group" aria-labelledby="keyboard-heading">
      <h2 id="keyboard-heading">Keyboard</h2>
      <div class="setting">
        <div class="setting-label"><label for="scroll-amount">d/u scroll distance (lines)</label>{@render help("scroll-amount-help", "About scroll distance", "Moves up to this many lines, limited to half the visible article.")}</div>
        <input id="scroll-amount" data-preference="scroll-amount" type="number" required min="1" max="100" step="1" aria-describedby="scroll-amount-help" value={Number(state.values["scroll-amount"])} onchange={change}>
      </div>
      <div class="setting shortcut-setting">
        <div class="setting-label"><label id="shortcuts-enable-label" for="single-key-shortcuts">Enable</label> <a id="show-shortcuts" data-no-swup href={actions.shortcutURL} aria-haspopup="dialog" aria-controls="shortcut-help" onclick={(event) => { event.preventDefault(); onShortcuts(); }}>keyboard shortcuts</a></div>
        <input id="single-key-shortcuts" data-preference="single-key-shortcuts" type="checkbox" aria-labelledby="shortcuts-enable-label show-shortcuts" checked={Boolean(state.values["single-key-shortcuts"])} onchange={change}>
      </div>
    </section>
    <section class="preferences-group" aria-labelledby="offline-heading">
      <h2 id="offline-heading">Offline reading</h2>
      <div class="setting">
        <div class="setting-label"><label for="offline-items">Recent articles to keep</label>{@render help("offline-items-help", "About offline reading", "Keep articles and their archived images on this device. Set to 0 to turn off automatic downloads. Embedded players need a connection.")}</div>
        <input id="offline-items" data-preference="offline-items" type="number" required min="0" max="1000" step="1" aria-describedby="offline-items-help" value={Number(state.values["offline-items"])} onchange={change}>
      </div>
      <p id="offline-download-status" class="muted" role="status" aria-live="polite">{state.offlineSummary}</p>
    </section>
    <section id="preferences-import" class="preferences-group preferences-import" aria-labelledby="preferences-import-heading" hidden={!state.pending}>
      <h2 id="preferences-import-heading">Review imported preferences</h2>
      <ul id="preferences-import-summary">{#each Object.entries(state.pending || {}) as [key, value]}<li>{summary(key, value)}</li>{/each}</ul>
      <div class="preference-actions"><button data-preferences-action="apply" onclick={() => actions.run("apply")} type="button">Apply preferences</button><button data-preferences-action="cancel" onclick={() => actions.run("cancel")} type="button">Cancel</button></div>
    </section>
    <section class="preferences-group preferences-transfer" aria-labelledby="preferences-transfer-heading">
      <div class="preferences-group-heading"><h2 id="preferences-transfer-heading">Transfer preferences</h2>{@render help("preferences-transfer-help", "About transferring preferences", "Use the same settings on another device. Links and files contain only the options above, never reading history.")}</div>
      <div class="preference-actions">
        <button id="share-state" data-preferences-action="share" onclick={() => actions.run("share")} type="button" hidden={!actions.canShare}>Share</button>
        <button id="copy-state" data-preferences-action="copy" onclick={() => actions.run("copy")} type="button">Copy link</button>
        <button data-preferences-action="save" onclick={() => actions.run("save")} type="button">Save file</button>
        <button data-preferences-action="import" onclick={() => actions.run("import")} type="button">Import file</button>
      </div>
      <div class="preference-actions preferences-reset"><button class="text-button" data-preferences-action="reset" onclick={() => actions.run("reset")} type="button">Reset to defaults</button><span id="preferences-status" class="muted" role="status" aria-live="polite">{state.status}</span></div>
      <input id="preferences-link" type="url" aria-label="Preferences link" readonly value={state.link} hidden={!state.link}>
      <input id="preferences-file" type="file" accept="application/json,.json" hidden onchange={fileChange}>
    </section>
