<script lang="ts">
  // `index.html`'s `<div class="listhead">`: the heading of a scoped list (a source with its
  // profile link, `/category` or `#tag`) and the source's error notice.
  import type { ClientScope } from '../generated/ClientScope';
  import type { ClientSourceError } from '../generated/ClientSourceError';
  import { formatDate } from '../dates/format';

  let {
    kind,
    scope,
    error,
  }: { kind: string; scope: ClientScope | null; error: ClientSourceError | null } = $props();

  const source = $derived(kind === 'source' && scope?.kind === 'source' ? scope : null);
  const category = $derived(
    (kind === 'category' || kind === 'tag') && scope && scope.kind === kind ? scope : null,
  );
</script>

{#if kind !== 'river' || error}
<div class="listhead">
  <h1>
    {#if source}{source.name}
      {#if source.site_url}<a class="muted" href={source.site_url} title={source.name} target="_blank" rel="noopener noreferrer">({source.profile})</a>{/if}
    {:else if category}{kind === 'tag' ? '#' : '/'}{category.name}{/if}
  </h1>
  <div class="listtools">
    {#if error}<span class="error" title={error.message}>⚠ {error.message} · since <time datetime={error.since} title={error.since}>{formatDate(error.since)}</time></span>{/if}
  </div>
</div>
{/if}
