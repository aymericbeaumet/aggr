<script lang="ts">
  // `item.html`'s `<footer class="article-footer">`: the next article and the recommendations.
  import type { ClientRow } from '../generated/ClientRow';
  import RelatedCard from '../feed/RelatedCard.svelte';

  let {
    next,
    recommended,
    base,
  }: { next: ClientRow | null; recommended: ClientRow[]; base: string } = $props();
</script>

{#if next || recommended.length}
<footer class="article-footer">
  {#if next}
  <section class="article-more-section" aria-labelledby="coming-next-title">
    <h2 class="article-more-heading" id="coming-next-title">Coming next</h2>
    <nav class="article-more" aria-labelledby="coming-next-title">
      <RelatedCard row={next} {base} comingNext={true} />
    </nav>
  </section>
  {/if}
  {#if recommended.length}
  <section class="article-more-section" aria-labelledby="discover-more-title">
    <h2 class="article-more-heading" id="discover-more-title">Discover more</h2>
    <nav class="article-more" aria-labelledby="discover-more-title">
      {#each recommended as row (row.path)}<RelatedCard {row} {base} comingNext={false} />{/each}
    </nav>
  </section>
  {/if}
</footer>
{/if}
