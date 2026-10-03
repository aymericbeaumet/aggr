<script lang="ts">
  // `_item.html`: one feed entry. Keep this structure aligned with search result rows.
  import type { ClientRow } from '../generated/ClientRow';
  import { ageBand } from '../dates/format';
  import { sitePath, urlFor } from '../model/urls';
  import { clock } from '../state/clock.svelte';
  import Metadata from './Metadata.svelte';
  import Preview from './Preview.svelte';

  let {
    row,
    rank,
    base,
    excerpts,
    language,
    hidden = false,
    selected = false,
    fresh = false,
    saved = false,
  }: {
    row: ClientRow;
    rank: number;
    base: string;
    excerpts: boolean;
    language: string;
    /** Outside the slice on screen (`?feed-page=`); the row stays in the document. */
    hidden?: boolean;
    /** Under the keyboard cursor. */
    selected?: boolean;
    /** Arrived since this session last saw the feed: lit as new. */
    fresh?: boolean;
    /** A search result whose page the worker has saved for offline reading. */
    saved?: boolean;
  } = $props();

  // `lang` marks a row only when it differs from the site's language.
  const lang = $derived(
    row.language && row.language.toLowerCase() !== language.toLowerCase() ? row.language : undefined,
  );
  const href = $derived(urlFor(base, row.url));
  // The band the build painted, recoloured as the article ages while the app runs.
  const published = $derived(Date.parse(row.metadata.date));
  const band = $derived(
    clock.now !== null && !Number.isNaN(published) ? ageBand(published, clock.now) : row.age_band,
  );
</script>

<li class="row h-entry age-{band}{selected ? ' is-selected' : ''}{fresh ? ' is-new' : ''}" data-age={band} data-path={row.path} data-url={sitePath(row.url)} data-link={row.link} {hidden}>
  <!-- svelte-ignore a11y_consider_explicit_label -->
  <a class="u-uid" {href} hidden></a>
  <div class="cell">
    <div class="row-content">
      <div class="row-copy">
        <div class="row-heading">
          <span class="rank" aria-hidden="true">{rank}.</span>
          <a class="title p-name u-url" data-row-open {href} {lang}>{row.title}</a>
        </div>
        {#if excerpts && row.excerpt}<p class="excerpt" {lang}>{row.excerpt}</p>{/if}
        <Metadata metadata={row.metadata} {base} />
        {#if saved}<div class="search-saved-status" data-saved-offline="true">Saved offline</div>{/if}
      </div>
      {#if row.preview}<Preview preview={row.preview} {base} />{/if}
    </div>
    {#if row.category}<data class="p-category" value={row.category} hidden></data>{/if}
    {#each row.labels as label (label.slug)}<data class="p-category" value={label.name} hidden></data>{/each}
  </div>
</li>
