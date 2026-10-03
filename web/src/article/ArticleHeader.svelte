<script lang="ts">
  // `item.html`'s chrome above the article content: identity, authors, the folding header,
  // resource links and the access notice. The `<article>` root belongs to the page; it is
  // where `language` matters (a `lang` attribute when the article's language differs).
  import type { ClientArticle } from '../generated/ClientArticle';
  import { facetUrl, urlFor } from '../model/urls';
  import Metadata from '../feed/Metadata.svelte';

  let {
    article,
    base,
    language,
  }: { article: ClientArticle; base: string; language: string } = $props();

  const header = $derived(article.header);
  // Words holding a `/` (paths, fractions) are wrapped so they can break and wrap well.
  const words = $derived(header.title.split(' '));
</script>

<!-- svelte-ignore a11y_consider_explicit_label -->
<a class="u-uid u-url" href={urlFor(base, header.url)} hidden></a>
{#each article.authors as author (author)}<data class="p-author h-card" value={author} hidden></data>{/each}
<header class="itemhead">
  <h1 class="p-name" tabindex="-1"><span class="itemhead-title">{#each words as word, index}{#if index}{' '}{/if}{#if word.includes('/')}<span class="itemhead-title-term">{word}</span>{:else}{word}{/if}{/each}</span></h1>
  <span class="itemhead-title-compact" aria-hidden="true">{header.title}</span>
  <Metadata metadata={header.metadata} {base} />
  {#if header.labels.length}<div class="item-tags" aria-label="Topics"><div class="item-tags-inner">{#each header.labels as label (label.slug)}<a class="tag p-category" rel="tag" href={facetUrl(base, label.slug, 'tag')}>#{label.name}</a>{/each}</div></div>{/if}
  <span class="itemhead-progress" aria-hidden="true"></span>
  <span class="itemhead-fade" aria-hidden="true"></span>
</header>
{#if article.resources.length}<nav class="article-resources" aria-label="Article resources"><span class="article-resources-label">Resources</span>{#each article.resources as resource (resource.url)}<a href={resource.url} title={resource.url} target="_blank" rel="noopener noreferrer">{resource.label} <span aria-hidden="true">↗</span></a>{/each}</nav>{/if}
{#if article.access.kind === 'subscription_required'}
<p class="empty article-access-notice">The publisher requires a subscription. <a href={header.link} title={header.link} target="_blank" rel="noopener noreferrer">Open the original</a>{#if article.access.archive_lookup_url}{' or '}<a href={article.access.archive_lookup_url} target="_blank" rel="noopener noreferrer">find an archived copy</a>{/if}.</p>
{:else if article.access.kind === 'titles_only'}
<p class="empty">This source publishes titles only — <a href={header.link} title={header.link} target="_blank" rel="noopener noreferrer">open the original</a>.</p>
{/if}
