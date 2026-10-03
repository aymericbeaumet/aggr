<script lang="ts">
  // `index.html`'s content block: the heading of a scoped list, the search shell, the rows and
  // the pager. In the browser the river is sliced by the page-size preference and the rows on
  // screen feed the keyboard cursor; a server render shows the complete static page.
  import type { ClientList } from '../generated/ClientList';
  import type { ClientPage } from '../generated/ClientPage';
  import type { ClientRow } from '../generated/ClientRow';
  import ListHead from '../feed/ListHead.svelte';
  import Pager from '../feed/Pager.svelte';
  import Row from '../feed/Row.svelte';
  import Toolbar from '../feed/Toolbar.svelte';
  import { sliceFeed } from '../feed/paging';
  import { sitePath } from '../model/urls';
  import { cursorRow } from '../selection/cursor';
  import { fresh } from '../state/fresh.svelte';
  import { page as shown } from '../state/page.svelte';
  import { preferences } from '../state/preferences.svelte';
  import { search } from '../state/search.svelte';
  import { selection } from '../state/selection.svelte';
  import { badges } from '../updates/badges';
  import { untrack } from 'svelte';

  let { page, list }: { page: ClientPage; list: ClientList } = $props();

  const kind = $derived(page.kind);
  // A scoped list seeds the search field with its own facet, exactly as the toolbar writes it.
  const scoped = $derived(list.scope && list.scope.kind === kind ? list.scope : null);
  const seed = $derived(scoped ? `${kind}:${scoped.slug} ` : '');
  // A live query replaces the feed: the results take the list region and the cursor.
  const searching = $derived(search.attached && search.active);
  const offset = $derived(list.paginator.offset);
  const indent = $derived(String(offset + list.rows.length).length - 1);
  const slicing = $derived(
    kind === 'river' && shown.href
      ? sliceFeed(list.paginator, list.rows.length, preferences.feedPageSize, shown.href, shown.root)
      : null,
  );
  const hidden = (index: number) => slicing !== null && (index < slicing.start || index >= slicing.end);
  const visible = $derived(slicing ? list.rows.slice(slicing.start, slicing.end) : list.rows);
  // The cursor is remembered by the item page's absolute URL, which only the app can resolve.
  // Hidden behind results, the feed holds no cursor: the results have it.
  const selected = (row: ClientRow) =>
    !searching && selection.url !== null && shown.root !== '' && selection.url === new URL(sitePath(row.url), shown.root).href;
  // An entry that arrived since this session last saw the feed, lit until the rows are replaced.
  const marked = (row: ClientRow) =>
    fresh.marked.length > 0 && shown.root !== '' && fresh.marked.includes(new URL(sitePath(row.url), shown.root).href);

  // A different slice of the list is a different set of rows: the cursor belongs on one of them,
  // unless the results have it; clearing the query hands it back.
  $effect(() => {
    const href = shown.href;
    if (href && !searching) selection.sync(visible.map((row) => cursorRow(row, shown.root)), href);
  });

  // The rows in front of the reader: the pending entries among them light up and are announced,
  // once the tab is visible and the feed is not hidden behind results.
  $effect(() => {
    const root = shown.root;
    if (!root || searching || !fresh.visible || !fresh.pending.length) return;
    const onScreen = visible.map((row) => new URL(sitePath(row.url), root).href);
    untrack(() => badges.reveal(onScreen));
  });

  // The list arrived at this address: the search field follows its `?q=`, and the search module
  // warms its catalogue (Pagefind itself waits for a valid query). Leaving ends the page's work.
  $effect(() => {
    const href = shown.href;
    const root = shown.root;
    const context = { seed, scope: scoped ? { kind, slug: scoped.slug } : null };
    if (!href) return;
    untrack(() => search.arrive({ root, href, ...context }));
    return () => untrack(() => search.leave());
  });

  // The stylesheet's own mark for a live query, on the body the navigation keeps in sync.
  $effect(() => {
    document.body.toggleAttribute('data-searching', searching);
    return () => document.body.removeAttribute('data-searching');
  });
</script>

{#if kind === 'river'}<h1 class="sr-only">{page.site.title} — independent web feed reader and archive</h1>{/if}
<ListHead {kind} scope={list.scope} error={list.error} />
<Toolbar {kind} scope={list.scope} base={page.base} />
<div data-static-feed hidden={searching}>
{#if list.rows.length}
<ol class="rows" role="list" start={offset + 1} style="--rank-indent: {indent}ch">
{#each list.rows as row, index (row.path)}<Row {row} rank={offset + index + 1} base={page.base} excerpts={page.site.excerpts} language={page.site.language} hidden={hidden(index)} selected={selected(row)} fresh={marked(row)} />{/each}
</ol>
{:else}
<p class="empty">Nothing here yet.</p>
{/if}
<Pager paginator={list.paginator} {kind} base={page.base} {slicing} />
</div>
