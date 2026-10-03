<script lang="ts">
  // `offline.html`'s content block, with the worker's report: the downloads' state and the
  // saved articles, each a link under the site root. The static page carries the same ids for
  // readers without JavaScript, where the list stays empty.
  import type { ClientPage } from '../generated/ClientPage';
  import { offline } from '../state/offline.svelte';
  import { page as shown } from '../state/page.svelte';
  import { offlineSummary } from './summary';

  let { page }: { page: ClientPage } = $props();

  const summary = $derived(offlineSummary(offline.status, page.site.pwa && offline.enabled));
  // Only pages of this site are offered; the worker never saves anything else, but the list is
  // what it says it is.
  const articles = $derived.by(() => {
    const root = shown.root;
    if (!root) return [];
    const list: { href: string; title: string }[] = [];
    for (const saved of offline.saved) {
      try {
        const url = new URL(saved.url, root);
        if (url.href.startsWith(root)) list.push({ href: url.href, title: saved.title });
      } catch {
        /* not a URL */
      }
    }
    return list;
  });
</script>

<div class="listhead"><h1>Offline</h1></div>
<p>This page has not been saved. Open one of your downloaded articles below, or reconnect and try again.</p>
<p id="offline-download-status">{summary}</p>
<p id="offline-empty" hidden={articles.length > 0}>No complete articles are available yet.</p>
<ul id="offline-articles">{#each articles as article (article.href)}<li><a href={article.href}>{article.title}</a></li>{/each}</ul>
