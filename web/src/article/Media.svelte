<script lang="ts">
  import type { ClientArticle } from '@aggr-model/ClientArticle';
  import { sourceSet, urlFor } from '../model/urls';
  let { article, base }: { article: ClientArticle; base: string } = $props();
  const media = $derived(article.media);
  const lead = $derived(article.lead);
  const preview = $derived(article.header.preview);
  const title = $derived(article.header.title);
  const seconds = $derived(article.header.metadata.consumption?.seconds || undefined);
  const show = $derived(article.header.metadata.feed_sources[0]?.name ?? article.header.metadata.source_title);
  const placeholder = $derived(preview ? `--image-placeholder: ${preview.color ?? 'transparent'}; --image-preview: url('${preview.placeholder.data_url}')` : undefined);
</script>
{#snippet poster()}
  {#if lead}<img src={urlFor(base, lead.url)} srcset={sourceSet(base, lead.srcset)} sizes="(max-width: 48rem) 100vw, 48rem" width={lead.width} height={lead.height} alt={lead.alt ?? ''} decoding="async" fetchpriority="high" />
  {:else if preview}<img src={urlFor(base, preview.url)} width={preview.width} height={preview.height} alt={preview.alt ?? ''} decoding="async" fetchpriority="high" />{/if}
{/snippet}
{#if media?.kind === 'document'}
  <figure class="document-reader" aria-label="PDF document"><div class="document-frame media-frame" style={placeholder}><span class="document-placeholder" aria-hidden="true"><svg viewBox="0 0 36 48"><path d="M5 2h17l9 9v35H5zM22 2v10h9M11 23h14M11 29h14M11 35h10" /></svg>PDF document</span><object class="document-viewer" data={urlFor(base, media.url)} type="application/pdf" title="{title} — PDF" width="768" height="720"><div class="document-fallback"><p>PDF preview unavailable</p><a href={urlFor(base, media.url)} target="_blank" rel="noopener noreferrer" aria-label="Open PDF: {title}">Open PDF ↗</a></div></object></div><figcaption><a href={urlFor(base, media.url)} target="_blank" rel="noopener noreferrer" aria-label="Open PDF: {title}">Open PDF ↗</a></figcaption></figure>
{:else if media?.kind === 'video'}
  <div class="video-player" data-video-provider={media.provider} data-duration-seconds={seconds} style={placeholder}><a class="video-preview" data-video-embed={media.embed_url} data-video-parent={media.requires_parent ? '' : undefined} href={article.header.link} title={article.header.link} target="_blank" rel="noopener noreferrer" aria-label="Play {title} on {media.title}">{#if lead || preview}{@render poster()}{:else}<span class="video-preview-label">{media.title}</span>{/if}<span class="video-preview-play" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M8 5v14l11-7z" /></svg></span></a></div><div class="media-timing" data-media-timing></div>
{:else if media?.kind === 'native_video'}
  <div class="native-video media-frame" style={placeholder}><!-- svelte-ignore a11y_media_has_caption --><video controls playsinline preload="none" data-duration-seconds={seconds} src={urlFor(base, media.url)} aria-label={title} width="1280" height="720" poster={preview ? urlFor(base, preview.url) : undefined}><a href={urlFor(base, media.url)} target="_blank" rel="noopener noreferrer">Open video</a></video></div><div class="media-timing" data-media-timing></div>
{:else if media?.kind === 'native_audio'}
  <div class="native-audio{preview ? ' has-artwork' : ''}" data-audio-player data-audio-component style={placeholder}>
    {#if preview}<div class="audio-cover"><img class="audio-artwork" src={urlFor(base, preview.url)} alt={preview.alt ?? ''} loading="lazy" decoding="async" /></div>{/if}
    <div class="audio-panel"><div class="audio-heading">{#if show && show !== title}<p class="audio-show">{show}</p>{/if}<p class="audio-episode">{title}</p><p class="audio-status" data-audio-status role="status" aria-live="polite"></p></div>
    <audio controls preload="none" data-duration-seconds={seconds} src={urlFor(base, media.url)} aria-label={title}><a href={urlFor(base, media.url)} target="_blank" rel="noopener noreferrer">Open audio</a></audio>
    <div class="audio-controls" data-audio-controls hidden><div class="audio-transport">
      {#each [30, 15] as amount}<button class="audio-skip" type="button" data-audio-skip={-amount} data-audio-back={amount === 15 ? '' : undefined} aria-label="Back {amount} seconds" title="Back {amount} seconds" disabled><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 4.5a8 8 0 1 1-6.93 4"/><path d="M14.5 2 12 4.5 14.5 7"/></svg><span aria-hidden="true">{amount}</span></button>{/each}
      <button class="audio-toggle" type="button" data-audio-toggle aria-label="Play" title="Play"><svg class="audio-play-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="m8 5 11 7-11 7z"/></svg><svg class="audio-pause-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M6 5h4v14H6zm8 0h4v14h-4z"/></svg></button>
      {#each [15, 30] as amount}<button class="audio-skip" type="button" data-audio-skip={amount} data-audio-forward={amount === 30 ? '' : undefined} aria-label="Forward {amount} seconds" title="Forward {amount} seconds" disabled><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 4.5a8 8 0 1 0 6.93 4"/><path d="M9.5 2 12 4.5 9.5 7"/></svg><span aria-hidden="true">{amount}</span></button>{/each}
    </div><div class="audio-timeline"><input type="range" min="0" max="1000" value="0" step="1" data-audio-seek aria-label="Playback position" disabled /><div class="audio-times"><span data-audio-elapsed>0:00</span><span class="audio-ends-at" data-audio-ends-at></span><span data-audio-duration>--:--</span></div></div></div>
    <div class="audio-tools" data-audio-tools hidden><label class="audio-speed">Speed <select data-audio-speed aria-label="Playback speed">{#each ['0.5','0.75','1','1.25','1.5','1.75','2','2.5','3'] as speed}<option value={speed} selected={speed === '1'}>{speed}×</option>{/each}</select></label><div class="audio-volume"><button type="button" data-audio-mute aria-label="Mute" title="Mute" aria-pressed="false"><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M11 5 6 9H3v6h3l5 4z"/><path class="audio-volume-wave" d="M15 8a6 6 0 0 1 0 8m3-11a10 10 0 0 1 0 14"/></svg></button><input data-audio-volume type="range" min="0" max="1" step="0.05" value="1" aria-label="Volume" /><span data-device-volume hidden title="Use the volume buttons on your device">Device volume</span></div></div>
    </div>
  </div>
{:else if media?.kind === 'interactive'}
  <figure class="interactive-reader" aria-label="Interactive page"><div class="interactive-frame media-frame"><iframe class="interactive-viewer" src={media.url} title="{title} — interactive original" sandbox="allow-scripts" referrerpolicy="no-referrer" loading="lazy" width="768" height="720"></iframe></div><figcaption><a href={media.url} target="_blank" rel="noopener noreferrer">Open original <span aria-hidden="true">↗</span></a></figcaption></figure>
{:else if lead}
  <figure class="article-lead media-frame" style="--media-ratio: {lead.width} / {lead.height}; --image-placeholder: {lead.color}; --image-preview: url('{lead.placeholder.data_url}')">{@render poster()}</figure>
{/if}
