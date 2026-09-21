<script lang="ts">
  import type { ItemMetadata } from './types';
  import { safeURL } from './display';
  import SearchDate from './SearchDate.svelte';
  import { facetURL } from './query';

  let { metadata, base, dateFormat, original, now = Date.now() }: {
    metadata: ItemMetadata; base: string; dateFormat: unknown; original: string; now?: number;
  } = $props();
</script>

<div class="meta">
  {#if metadata.source_display}<span class="meta-field"><span class="domain"><a href={facetURL(base, 'source', metadata.source_query || metadata.source_slug || '')} title={metadata.source_title || metadata.source_display}><span class="source-resolved">{metadata.source_display}</span></a>{#if metadata.feed_sources?.length}{' '}<em>via {#each metadata.feed_sources as feed, i}{#if i > 0}, {/if}<a class="source-feed" href={facetURL(base, 'source', feed.query_value || feed.slug)} title={feed.name}>{feed.display}</a>{/each}</em>{/if}</span></span>{/if}
  {#if metadata.category}<span class="meta-field"><span class="category"><a class="p-category" rel="tag" href={facetURL(base, 'category', metadata.category.slug)}>/{metadata.category.name}</a></span></span>{/if}
  {#if metadata.date}<span class="meta-field"><SearchDate published={metadata.date} updated={metadata.updated} {dateFormat} {now} /></span>{/if}
  {#if metadata.consumption}{@const consumption = metadata.consumption}<span class="meta-field"><span class="reading-stats" data-consumption={consumption.action} data-duration-seconds={consumption.seconds} title={consumption.words ? `${consumption.words} ${consumption.words === 1 ? 'word' : 'words'}` : undefined}>{#if consumption.minutes}<time datetime={`PT${consumption.seconds || consumption.minutes}${consumption.seconds ? 'S' : 'M'}`}>{consumption.minutes} min {consumption.action}</time>{:else}{consumption.action === 'listen' ? 'Listen' : 'Watch'}{/if}</span></span>{/if}
  <span class="meta-field"><a class="u-bookmark-of" href={original} title={original} target="_blank" rel="external noopener noreferrer via">original</a></span>
  {#each metadata.discussions || [] as discussion}<span class="meta-field"><a class="discussion" data-discussion={discussion.name} href={safeURL(discussion.url, base)} title={discussion.url} target="_blank" rel="noopener noreferrer" aria-label={`${discussion.name}, matching discussion found${discussion.score !== undefined ? `, score ${discussion.score}` : ''}`}>{discussion.name}</a></span>{/each}
  {#if metadata.points !== undefined}<span class="meta-field"><span>{metadata.points} points</span></span>{/if}
  {#if metadata.comments}<span class="meta-field"><a href={safeURL(metadata.comments.url, base)} title={metadata.comments.url} target="_blank" rel="noopener noreferrer">{metadata.comments.count !== undefined ? `${metadata.comments.count} comments` : 'comments'}</a></span>{/if}
</div>
