<script lang="ts">
  import type { ClientArticle } from '@aggr-model/ClientArticle';
  import type { ClientPage } from '@aggr-model/ClientPage';
  import ArticleFooter from '../article/ArticleFooter.svelte';
  import ArticleHeader from '../article/ArticleHeader.svelte';
  import { createRawSnippet } from 'svelte';
  import Media from '../article/Media.svelte';
  import { enhance } from '../article/enhance';
  import { swipeable } from '../article/swipes';
  import { sitePath } from '../model/urls';

  let {
    page,
    article,
    bodyHtml = '',
  }: { page: ClientPage; article: ClientArticle; bodyHtml?: string } = $props();

  const body = createRawSnippet<[string, boolean]>((html, marginNotes) => ({
    render: () => `<div class="body e-content${marginNotes() ? ' has-margin-notes' : ''}" data-article-body>${html()}</div>`,
  }));
  const header = $derived(article.header);
  const lang = $derived(
    header.language && header.language.toLowerCase() !== page.site.language.toLowerCase()
      ? header.language
      : undefined,
  );
</script>

<article class="item h-entry" {lang} data-path={header.path} data-url={sitePath(header.url)} data-link={header.link} data-previous-url={article.previous ? sitePath(article.previous.url) : undefined} data-next-url={article.next ? sitePath(article.next.url) : undefined} {@attach swipeable}>
  <ArticleHeader {article} base={page.base} language={page.site.language} />
  <div data-article-content {@attach enhance({ next: article.next, previous: article.previous })}>
    <Media {article} base={page.base} />
    {@render body(bodyHtml, article.has_margin_notes)}
  </div>
  <ArticleFooter next={article.next} recommended={article.recommended} base={page.base}  />
</article>
