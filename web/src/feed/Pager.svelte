<script lang="ts">
  // `index.html`'s `<nav class="pager">`. The river's pager also carries the static page
  // geometry the client slices the feed with, and hides itself when there is a single page.
  import type { PaginatorCtx } from '../generated/PaginatorCtx';
  import { sitePath, urlFor } from '../model/urls';
  import type { Slicing } from './paging';

  let {
    paginator,
    kind,
    base,
    slicing = null,
  }: {
    paginator: PaginatorCtx;
    kind: string;
    base: string;
    /** The river's client-side slice, whose links replace the static page geometry. */
    slicing?: Slicing | null;
  } = $props();

  const river = $derived(kind === 'river');
  const previous = $derived(paginator.previous);
  const next = $derived(paginator.next);
</script>

{#if paginator.number_pagers > 1 || river}
<nav
  class="pager"
  aria-label="Pagination"
  data-feed-pager={river ? '' : undefined}
  data-static-page={river ? paginator.current_index : undefined}
  data-static-pages={river ? paginator.number_pagers : undefined}
  data-static-page-size={river ? paginator.paginate_by : undefined}
  data-total-items={river ? paginator.total_items : undefined}
  data-static-first={river ? sitePath(paginator.first) : undefined}
  data-static-last={river ? sitePath(paginator.last) : undefined}
  data-static-previous={river && previous ? sitePath(previous) : undefined}
  data-static-next={river && next ? sitePath(next) : undefined}
  hidden={slicing ? slicing.hidden : river && paginator.number_pagers === 1}
>
  <span class="pager-side">
    <a data-page-first href={slicing ? slicing.first.href : urlFor(base, paginator.first)} hidden={slicing ? !slicing.first.visible : !previous}>first</a>
    <a data-page-previous href={slicing ? slicing.previous.href : urlFor(base, previous ?? paginator.first)} hidden={slicing ? !slicing.previous.visible : !previous}>newer</a>
  </span>
  <span data-page-status>{slicing ? slicing.status : `page ${paginator.current_index} / ${paginator.number_pagers}`}</span>
  <span class="pager-side">
    <a data-page-next href={slicing ? slicing.next.href : urlFor(base, next ?? paginator.last)} hidden={slicing ? !slicing.next.visible : !next}>older</a>
    <a data-page-last href={slicing ? slicing.last.href : urlFor(base, paginator.last)} hidden={slicing ? !slicing.last.visible : !next}>last</a>
  </span>
</nav>
{/if}
