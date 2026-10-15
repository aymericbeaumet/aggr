<script lang="ts">
  // `_related_item.html`: the "coming next" and "discover more" cards under an article.
  import type { ClientRow } from '../generated/ClientRow';
  import { urlFor } from '../model/urls';
  import Metadata from './Metadata.svelte';
  import Preview from './Preview.svelte';

  let {
    row,
    base,
    comingNext,
  }: { row: ClientRow; base: string; comingNext: boolean } = $props();
</script>

<div class="article-more-card h-entry">
  <div class="row-content">
    <div class="row-copy">
      <a class="article-more-link title" rel={comingNext ? 'next' : undefined} href={urlFor(base, row.url)}>{row.title}</a>
      <Metadata metadata={row.metadata} {base} />
    </div>
    {#if row.preview}<Preview preview={row.preview} {base} />{/if}
  </div>
</div>
