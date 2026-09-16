<script lang="ts">
  import { Command } from 'bits-ui';
  import type { Readable } from 'svelte/store';
  import type { ViewState } from './state';
  import { untrack } from 'svelte';
  import { completionHighlight, selectedCompletion, type Completion } from './completion';

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
  // Set once the user drives the highlight (arrows or pointer) so store emissions do not snap it back to the first item.
  let moved = $state(false);
  let seenQuery = '';
  let hovering = $state(false), helpDismissed = $state(false);
  const helpVisible = $derived(hovering && !helpDismissed);
  // Only a new suggestions array may re-render the menu, and each item's value must stay a stable derived id:
  // bits-ui re-registers an item whenever its `value` prop is dirtied and then re-selects the first entry after
  // the tick, which is what snapped an arrowed-to highlight back on every unrelated store emission.
  const suggestions = $derived($model.suggestions);
  $effect(() => { value = $model.query; });
  $effect(() => {
    const { open, query, suggestions } = $model;
    untrack(() => {
      if (query !== seenQuery || !open || !suggestions.length) moved = false;
      seenQuery = query;
      // `selected` is the live Command.Root binding (bits-ui writes it synchronously on every arrow or pointer move).
      // A moved highlight that is still listed is never echoed back, so an emission cannot restore a previous item.
      const highlight = completionHighlight(suggestions, selected, moved);
      if (highlight !== undefined && highlight !== selected) selected = highlight;
    });
  });

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
    if (event.key === 'Escape') {
      // First Escape closes open suggestions; the next one leaves the field.
      event.preventDefault(); event.stopPropagation(); helpDismissed = true;
      const suggesting = $model.open && $model.suggestions.length > 0;
      dismiss();
      if (!suggesting) input?.blur();
      return;
    }
    if ((event.key === 'Enter' || (event.key === 'Tab' && !event.shiftKey)) && $model.open && $model.suggestions.length) {
      event.preventDefault(); event.stopPropagation();
      const choice = selectedCompletion($model.suggestions, selected);
      if (choice) choose(choice);
      return;
    }
    // Without suggestions the arrows walk the results and Enter opens the selected one.
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      if (move(event.key === 'ArrowDown' ? 1 : -1)) { event.preventDefault(); event.stopPropagation(); }
      else if ($model.open && $model.suggestions.length) moved = true;
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
        {#snippet child({ props })}<input {...props} bind:value aria-expanded={$model.open && $model.suggestions.length > 0} aria-controls={$model.open && $model.suggestions.length > 0 ? 'search-completions' : undefined} />{/snippet}
      </Command.Input>
      {#if $model.query}<button type="button" class="search-clear" aria-label="Clear search" onclick={() => { clear(); input?.focus({ preventScroll: true }); }}>×</button>{/if}
    </div>
  </form>
  {#if $model.open && $model.suggestions.length}
    <Command.List id="search-completions" class="search-completions" aria-label="Search suggestions">
      <Command.Viewport>
        {#each suggestions as suggestion (suggestion.id)}
          {@const id = suggestion.id}
          <Command.Item value={id} onSelect={() => choose(suggestion)} onpointermove={() => { moved = true; }} class="search-completion" data-completion-id={id}>
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

