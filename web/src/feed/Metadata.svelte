<script lang="ts">
  // `_metadata.html`: the metadata line shared by feed rows, article headers and related
  // cards. The fields sit on one line with no whitespace of their own; the spaces around the
  // published date and the original link are the template's.
  import type { Metadata } from '../generated/Metadata';
  import { formatDate, formatTimestamp, localizedTooltip, publishedTooltip } from '../dates/format';
  import { facetPage, facetUrl } from '../model/urls';
  import { clock } from '../state/clock.svelte';
  import { mediaDurations } from '../state/media.svelte';
  import { preferences } from '../state/preferences.svelte';

  let {
    metadata,
    base,
    dateFormat = undefined,
  }: {
    metadata: Metadata;
    base: string;
    /** The `date-format` style; defaults to the loaded preference, and to the template's day. */
    dateFormat?: string;
  } = $props();

  const updated = $derived(metadata.updated);
  // A loaded native player corrects the archived length (`docs/rendering.md`).
  const consumption = $derived(mediaDurations.correct(metadata.consumption, metadata.original));
  const style = $derived(dateFormat ?? preferences.dateFormat);
  const timestamp = $derived(Date.parse(metadata.date));
  const updatedAt = $derived(updated ? Date.parse(updated) : Number.NaN);
  // The template's day until the app runs; then the preferred style, kept current by the clock.
  const live = $derived(style !== undefined && clock.now !== null && !Number.isNaN(timestamp));
  const dateText = $derived(
    (live ? formatTimestamp(timestamp, style as string, clock.now as number) : null) ?? formatDate(metadata.date),
  );
  // The exact, localized timestamp costs a formatter; build it only when someone looks.
  let looked = $state(false);
  const publishedTitle = $derived(
    looked && live
      ? localizedTooltip(timestamp, Number.isNaN(updatedAt) ? null : updatedAt)
      : publishedTooltip(metadata.date, updated),
  );
  const look = () => {
    looked = true;
  };
  const wordsTitle = $derived(
    consumption?.words ? `${consumption.words} ${consumption.words === 1 ? 'word' : 'words'}` : undefined,
  );
</script>

<div class="meta">
  {#if metadata.source_display}<span class="meta-field"><span class="domain"><a href={facetPage(base, metadata.source_query || metadata.source_slug, 'source')} title={metadata.source_title}><span class="source-resolved">{metadata.source_display}</span></a>{#if metadata.feed_sources.length}{' '}<em>via {#each metadata.feed_sources as feed, index (feed.slug)}{#if index}{', '}{/if}<a class="source-feed" href={facetPage(base, feed.query_value || feed.slug, 'source')} title={feed.name}>{feed.display}</a>{/each}</em>{/if}</span></span>{/if}{#if metadata.category}<span class="meta-field"><span class="category"><a class="p-category" rel="tag" href={facetUrl(base, metadata.category.slug, 'category')}>/{metadata.category.name}</a></span></span>{/if}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <span class="meta-field"><span class="published-date" data-date-tooltip data-date-updated={updated} title={publishedTitle} onpointerenter={look} onfocusin={look}><time class="dt-published" datetime={metadata.date} aria-label={live ? `${dateText}; ${publishedTitle}` : undefined}>{dateText}</time></span></span>
  {#if consumption}<span class="meta-field"><span class="reading-stats" data-consumption={consumption.action} data-duration-seconds={consumption.seconds || undefined} title={wordsTitle}>{#if consumption.minutes}<time datetime="PT{consumption.seconds ? consumption.seconds : consumption.minutes}{consumption.seconds ? 'S' : 'M'}">{consumption.minutes} min {consumption.action}</time>{:else}{consumption.action === 'listen' ? 'Listen' : 'Watch'}{/if}</span></span>{/if}
  <span class="meta-field"><a class="u-bookmark-of" href={metadata.original} title={metadata.original} target="_blank" rel="external noopener noreferrer via">original</a></span>
  {#each metadata.discussions as discussion (discussion.name)}<span class="meta-field"><a class="discussion" data-discussion={discussion.name} href={discussion.url} title={discussion.url} target="_blank" rel="noopener noreferrer" aria-label="{discussion.name}, matching discussion found{discussion.score !== undefined ? `, score ${discussion.score}` : ''}">{discussion.name}</a></span>{/each}{#if metadata.points !== undefined}<span class="meta-field"><span>{metadata.points} points</span></span>{/if}{#if metadata.comments}<span class="meta-field"><a href={metadata.comments.url} title={metadata.comments.url} target="_blank" rel="noopener noreferrer">{#if metadata.comments.count !== undefined}{metadata.comments.count}{' '}{/if}comments</a></span>{/if}
</div>
