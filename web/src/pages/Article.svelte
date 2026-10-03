<script lang="ts">
  // `item.html`'s content block: the article root with its chrome, the adopted content region
  // (media figure and body, moved from the document rather than re-parsed) and the footer cards.
  // The region's behaviour (footnotes, headings, sharing, swipes, players) is attached after the
  // nodes are moved in and ends with the wrapper, which the page key rebuilds on navigation.
  import type { ClientArticle } from '../generated/ClientArticle';
  import type { ClientPage } from '../generated/ClientPage';
  import ArticleFooter from '../article/ArticleFooter.svelte';
  import ArticleHeader from '../article/ArticleHeader.svelte';
  import { adopt } from '../article/adopt';
  import { enhance } from '../article/enhance';
  import { swipeable } from '../article/swipes';
  import { sitePath } from '../model/urls';

  let {
    page,
    article,
    content = null,
  }: { page: ClientPage; article: ClientArticle; content?: Element | null } = $props();

  const header = $derived(article.header);
  const lang = $derived(
    header.language && header.language.toLowerCase() !== page.site.language.toLowerCase()
      ? header.language
      : undefined,
  );
</script>

<article class="item h-entry" {lang} data-path={header.path} data-url={sitePath(header.url)} data-link={header.link} data-previous-url={article.previous ? sitePath(article.previous.url) : undefined} data-next-url={article.next ? sitePath(article.next.url) : undefined} {@attach swipeable}>
  <ArticleHeader {article} base={page.base} language={page.site.language} />
  <div data-article-content {@attach adopt(content)} {@attach content ? enhance({ next: article.next, previous: article.previous }) : null}></div>
  <ArticleFooter next={article.next} recommended={article.recommended} base={page.base} excerpts={page.site.excerpts} />
</article>
