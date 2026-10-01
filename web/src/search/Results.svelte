<script lang="ts">
  // The `[data-search-results]` region of `_feed_toolbar.html`: the live status, the result
  // rows (the same `Row` the feed draws, keyed by URL so a refresh keeps the rows that stay),
  // the empty message and the pager. Static and hidden until the state has results.
  import type { ClientRow } from '../generated/ClientRow';
  import Row from '../feed/Row.svelte';
  import { sitePath } from '../model/urls';
  import { cursorRow } from '../selection/cursor';
  import { offline } from '../state/offline.svelte';
  import { page as shown } from '../state/page.svelte';
  import { search } from '../state/search.svelte';
  import { selection } from '../state/selection.svelte';

  let { base }: { base: string } = $props();

  const results = $derived(search.attached && search.active ? search.results : null);
  const rows = $derived(results?.rows ?? []);
  const offset = $derived(results ? (results.page - 1) * results.size : 0);
  const indent = $derived(String(offset + rows.length).length - 1);
  const excerpts = $derived(shown.model?.site.excerpts ?? true);
  const language = $derived(shown.model?.site.language ?? '');
  const status = $derived(results || search.busy ? search.status : '');
  /** Pages the worker holds complete for offline reading, by their absolute address. */
  const savedPages = $derived(
    new Set(shown.root === '' ? [] : offline.saved.map((article) => new URL(sitePath(article.url), shown.root).href)),
  );
  const saved = (row: ClientRow) => shown.root !== '' && savedPages.has(new URL(sitePath(row.url), shown.root).href);
  const selected = (row: ClientRow) =>
    selection.url !== null && shown.root !== '' && selection.url === new URL(sitePath(row.url), shown.root).href;
  /** The address of another page of these results. */
  const pageHref = (number: number): string => {
    if (!results || !search.address) return '#';
    const url = new URL(search.address);
    if (number > 1) url.searchParams.set('search-page', String(number));
    else url.searchParams.delete('search-page');
    return url.href;
  };
  let list: HTMLOListElement | undefined = $state();

  // The results own the cursor while they are on screen, remembered by their own address.
  $effect(() => {
    if (!results) return;
    const address = search.address || shown.href;
    if (address) selection.sync(rows.map((row) => cursorRow(row, shown.root)), address);
  });

  // Results may arrive in stages: a title that had the keyboard keeps it, found again by URL.
  let focused = '';
  $effect.pre(() => {
    void rows;
    const active = document.activeElement;
    focused = list && active instanceof HTMLAnchorElement && list.contains(active) ? active.href : '';
  });
  $effect(() => {
    void rows;
    if (!focused || !list) return;
    const link = Array.from(list.querySelectorAll<HTMLAnchorElement>('[data-row-open]')).find((candidate) => candidate.href === focused);
    if (link && document.activeElement !== link) link.focus({ preventScroll: true });
  });

  function turn(event: MouseEvent, delta: number): void {
    event.preventDefault();
    search.drive((driver) => driver.turn(delta));
  }
</script>

<div data-search-results data-nosnippet>
  <p id="search-status" role="status" aria-live="polite" hidden={!status}>{status}</p>
  <ol id="list" class="rows search-results" role="list" aria-busy={results && search.busy ? 'true' : 'false'} hidden={!results} style={results ? `--rank-indent: ${indent}ch` : undefined} bind:this={list}>{#each rows as row, index (row.url)}<Row {row} rank={offset + index + 1} {base} {excerpts} {language} selected={selected(row)} saved={saved(row)} />{/each}</ol>
  <p id="empty" hidden={!results || results.total !== 0}>No articles match this search. Try fewer filters or different words.</p>
  <nav class="pager search-pager" aria-label="Search results pages" hidden={!results || results.pages <= 1}>
    <span class="pager-side"><a data-search-page-previous href={pageHref((results?.page ?? 1) - 1)} hidden={!results || results.page <= 1} onclick={(event) => turn(event, -1)}>newer</a></span>
    <span data-search-page-status>{results && results.pages > 1 ? `page ${results.page} / ${results.pages}` : ''}</span>
    <span class="pager-side"><a data-search-page-next href={pageHref((results?.page ?? 1) + 1)} hidden={!results || results.page >= results.pages} onclick={(event) => turn(event, 1)}>older</a></span>
  </nav>
</div>
