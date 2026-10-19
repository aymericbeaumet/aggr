<script lang="ts">
  // The `#search-completions` listbox: empty and hidden on the static page, filled from the
  // state. The field keeps the keyboard (`aria-activedescendant` names the highlight); a pointer
  // chooses on press so the field is never blurred on the way.
  import type { Completion } from './completion';
  import { search } from '../state/search.svelte';

  const shown = $derived(search.suggesting ? search.completions : []);
  let list: HTMLUListElement | undefined = $state();

  const detail = (completion: Completion) =>
    (completion.detail + (completion.count !== undefined ? ` · ${completion.count}` : '')).trim();

  function choose(completion: Completion): void {
    search.driver?.choose(completion);
  }

  function highlight(index: number): void {
    if (search.driver) search.driver.highlight(index);
    else search.highlighted = index;
  }

  // A highlight that moved by keyboard stays in view of a long list.
  $effect(() => {
    const index = search.highlighted;
    if (!shown.length || !list) return;
    const item = list.children[index];
    if (item instanceof HTMLElement) item.scrollIntoView?.({ block: 'nearest' });
  });
</script>

<ul id="search-completions" class="search-completions" role="listbox" aria-label="Search suggestions" hidden={!shown.length} bind:this={list}>{#each shown as completion, index (completion.id)}<li class="search-completion" role="option" id="search-completion-{index}" data-completion-id={completion.id} aria-selected={index === search.highlighted} data-selected={index === search.highlighted ? '' : undefined} onpointerdown={(event) => { event.preventDefault(); choose(completion); }} onpointermove={() => highlight(index)}><span class="completion-label">{completion.label}</span>{#if detail(completion)}<small>{detail(completion)}</small>{/if}</li>{/each}</ul>
