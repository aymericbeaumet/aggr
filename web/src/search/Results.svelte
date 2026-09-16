<script lang="ts">
  import type { Readable } from 'svelte/store';
  import type { ViewState } from './state';
  import { displayData, resultExcerptParts, safeURL } from './display';
  import Metadata from './Metadata.svelte';
  import Preview from './Preview.svelte';

  let { model, pageChanged }: { model: Readable<ViewState>; pageChanged: (page: number) => void } = $props();
</script>

{#if $model.active}
  <p id="search-status" role="status" aria-live="polite">{#if $model.busy && !$model.ready}Searching…{:else if $model.ready}{$model.page.total} {$model.page.total === 1 ? 'article' : 'articles'}{/if}</p>
{/if}
{#if $model.ready && $model.active}
  <ol id="list" class="rows search-results" role="list" aria-busy="false" style:--rank-indent={`${Math.max(2, String($model.page.total).length)}ch`}>
    {#each $model.page.results as entry, index (entry.url)}
      {@const display = displayData(entry)}
      {@const page = safeURL(entry.url, $model.base)}
      {@const original = safeURL(display.original || entry.url, $model.base)}
      {@const preview = display.preview}
      <li class="row h-entry" class:is-selected={page === $model.selectedURL || (!$model.selectedURL && index === 0)} data-url={page} data-link={original}>
        <a class="u-uid u-url" href={page} hidden aria-label={entry.meta.title}></a>
        <div class="cell"><div class="row-content"><div class="row-copy">
          <div class="row-heading"><span class="rank" aria-hidden="true">{($model.page.page - 1) * $model.page.size + index + 1}.</span><a class="title p-name u-url" data-row-open href={page}>{entry.meta.title || 'Untitled'}</a></div>
          <div class="search-excerpt">{#each resultExcerptParts(entry) as part}{#if part.highlight}<mark>{part.text}</mark>{:else}{part.text}{/if}{/each}</div>
          <Metadata metadata={display} base={$model.base} dateFormat={$model.dateFormat} now={$model.now} {original} />
          {#if ($model.hasOfflineStatus || $model.isOffline) && $model.savedURLs.has(page)}<div class="search-saved-status" data-saved-offline="true">Saved offline</div>{/if}
        </div>
        {#if preview && (!$model.isOffline || $model.cachedPreviews.has(safeURL(preview.url, $model.base)))}<Preview {preview} src={safeURL(preview.url, $model.base)} />{/if}
        </div></div>
      </li>
    {/each}
  </ol>
  {#if !$model.page.total}<p id="empty">No articles match this search. Try fewer filters or different words.</p>{/if}
  {#if $model.page.pages > 1}
    <nav class="pager" aria-label="Search results pages">
      <button type="button" disabled={$model.page.page <= 1} onclick={() => pageChanged($model.page.page - 1)}>Previous</button>
      <span>page {$model.page.page} / {$model.page.pages}</span>
      <button type="button" disabled={$model.page.page >= $model.page.pages} onclick={() => pageChanged($model.page.page + 1)}>Next</button>
    </nav>
  {/if}
{/if}

<style>
  .search-saved-status { color: var(--muted); font-size: .75rem; padding-left: calc(1.25rem + .5rem); margin-top: .2rem; }
</style>
