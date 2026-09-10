<script lang="ts">
  import { Command } from 'bits-ui';
  import type { Readable } from 'svelte/store';
  import type { ViewState } from './state';
  import { selectedCompletion, type Completion } from './completion';

  let { model, change, caret, choose, submit, move, open, clear, focusInput, revealInput, dismiss, blur }: {
    model: Readable<ViewState>;
    change: (query: string, cursor: number, composing: boolean) => void;
    caret: (cursor: number) => void; choose: (item: Completion) => void;
    submit: () => void; move: (direction: number) => boolean; open: () => boolean;
    clear: () => void; focusInput: () => void; revealInput: () => void; dismiss: () => void; blur: () => void;
  } = $props();
  let input: HTMLInputElement | null = $state(null);
  let composing = $state(false);
  let value = $state('');
  let selected = $state('');
  let hovering = $state(false), helpDismissed = $state(false);
  const helpVisible = $derived(hovering && !helpDismissed);
  $effect(() => { value = $model.query; });
  $effect(() => { selected = selectedCompletion($model.suggestions, selected)?.id || ''; });

  function inputChanged(event: Event) {
    const target = event.currentTarget as HTMLInputElement;
    change(target.value, target.selectionStart ?? target.value.length, composing);
  }
  function updateCaret(event: Event) {
    if (event instanceof KeyboardEvent && !['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    if (input) caret(input.selectionStart ?? input.value.length);
  }
  function keydown(event: KeyboardEvent) {
    if (composing || event.isComposing || event.keyCode === 229) { event.stopPropagation(); return; }
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); helpDismissed = true; dismiss(); input?.blur(); }
    if ((event.key === 'Enter' || (event.key === 'Tab' && !event.shiftKey)) && $model.open && $model.suggestions.length) {
      event.preventDefault(); event.stopPropagation();
      const choice = selectedCompletion($model.suggestions, selected);
      if (choice) choose(choice);
      return;
    }
    // Without suggestions the arrows walk the results and Enter opens the selected one.
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      if (move(event.key === 'ArrowDown' ? 1 : -1)) { event.preventDefault(); event.stopPropagation(); }
      return;
    }
    if (event.key === 'Enter' && (!$model.open || !$model.suggestions.length)) {
      event.preventDefault(); event.stopPropagation();
      if (!open()) submit();
    }
  }
</script>

<div class="search-control" role="group" aria-label="Search controls" onpointerenter={(event) => { hovering = event.pointerType === 'mouse'; if (hovering) helpDismissed = false; }} onpointerleave={() => { hovering = false; }}>
<Command.Root bind:value={selected} shouldFilter={false} vimBindings={false} label="Search articles" class="search-command">
  <form id="search-form" role="search" onsubmit={(event) => { event.preventDefault(); submit(); }}>
    <label class="sr-only" for="q">Search articles</label>
    <div class="search-input-line">
      <Command.Input id="q" bind:ref={input} bind:value type="search" name="q" autocomplete="off" spellcheck="false"
        placeholder="Search articles, sources, categories…" aria-describedby="search-query-help" aria-keyshortcuts="Meta+K Control+K"
        oninput={inputChanged} onkeydown={keydown} onkeyup={updateCaret} onclick={(event) => { revealInput(); updateCaret(event); }} onselect={updateCaret}
        onfocus={focusInput} onblur={() => { window.setTimeout(() => { if (!input?.closest('.search-command')?.contains(document.activeElement)) blur(); }, 100); }}
        oncompositionstart={() => { composing = true; }}
        oncompositionend={(event) => { composing = false; inputChanged(event); }}>
        {#snippet child({ props })}<input {...props} bind:value aria-expanded={$model.open && $model.suggestions.length > 0} />{/snippet}
      </Command.Input>
      {#if $model.query}<button type="button" class="search-clear" aria-label="Clear search" onclick={() => { clear(); input?.focus({ preventScroll: true }); }}>×</button>{/if}
    </div>
  </form>
  {#if $model.open && $model.suggestions.length}
    <Command.List class="search-completions" aria-label="Search suggestions">
      <Command.Viewport>
        {#each $model.suggestions as suggestion (suggestion.id)}
          <Command.Item value={suggestion.id} onSelect={() => choose(suggestion)} class="search-completion" data-completion-id={suggestion.id}>
            <span class="completion-label">{suggestion.label}</span>
            {#if suggestion.detail || suggestion.count !== undefined}<small>{suggestion.detail}{suggestion.count !== undefined ? ` · ${suggestion.count}` : ''}</small>{/if}
          </Command.Item>
        {/each}
      </Command.Viewport>
    </Command.List>
  {/if}
</Command.Root>
<div id="search-query-help" class="search-query-help" role="tooltip" hidden={!helpVisible}>
  <span>Use <code>source:</code>, <code>category:</code>, <code>tag:</code>, <code>type:</code>, <code>date:</code>, <code>sort:</code>, <code>"exact phrases"</code> or <code>-exclude</code>.</span>
</div>
</div>
{#if $model.error}<p class="search-error" role="alert">{$model.error}</p>{/if}
{#if $model.offline}<p class="search-offline" role="status">{$model.offline}</p>{/if}

<style>
  .search-control { position: relative; }
  :global(.search-command) { position: relative; width: 100%; min-width: 0; }
  form { margin: 0; width: 100%; }
  .search-input-line { position: relative; width: 100%; }
  :global(.search-input-line #q) { box-sizing: border-box; width: 100%; min-width: 0; min-height: 44px; padding: .55rem 2.75rem .55rem .7rem; border: 1px solid var(--faint); border-radius: .35rem; color: var(--fg); background: var(--search-bg, var(--code)); font: inherit; }
  .search-clear { position: absolute; inset-block: 0; right: 0; display: grid; place-items: center; width: 44px; min-height: 44px; padding: 0; border: 0; border-radius: .35rem; color: var(--muted); background: transparent; font-size: 1.5rem; line-height: 1; }
  :global(.search-completions) { position: absolute; z-index: 20; top: 100%; inset-inline: 0; max-height: min(24rem, 55vh); overflow-y: auto; border: 1px solid var(--faint); border-radius: .4rem; background: var(--bg); box-shadow: 0 6px 20px #0002; padding: .3rem; }
  :global(.search-completion) { display: flex; justify-content: space-between; align-items: center; gap: .75rem; min-height: 44px; padding: .4rem .65rem; border-radius: .25rem; cursor: pointer; }
  :global(.search-completion[data-selected]) { background: var(--code); }
  .completion-label { min-width: 0; overflow-wrap: anywhere; }
  small { flex: none; color: var(--muted); }
  .search-query-help { position: absolute; z-index: 21; bottom: calc(100% + .4rem); inset-inline-start: 0; box-sizing: border-box; max-width: min(38rem, 100%); padding: .5rem .65rem; border: 1px solid var(--faint); border-radius: .35rem; color: var(--muted); background: var(--bg); box-shadow: 0 3px 12px #0002; font-size: .75rem; line-height: 1.5; }
  .search-query-help[hidden] { display: none; }
  .search-error, .search-offline { font-size: .85rem; margin: .4rem 0; }
  .search-error { color: var(--warm); }
</style>
