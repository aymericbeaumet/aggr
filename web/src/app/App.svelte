<script lang="ts">
  // The reader inside `<div id="app">`: `base.html`'s persistent header and tab bar, the page's
  // `<main>`, and the footer. Lists and articles are rendered from the model; a static page's
  // `<main>` children are adopted from the document as they are. Rendered on the server for the
  // whole-page parity fixtures, where nothing is adopted and the wrappers stay empty.
  import type { ClientPage } from '../generated/ClientPage';
  import { adopt } from '../article/adopt';
  import Offline from '../offline/Offline.svelte';
  import Article from '../pages/Article.svelte';
  import Feed from '../pages/Feed.svelte';
  import Preferences from '../pages/Preferences.svelte';
  import { page as shown } from '../state/page.svelte';
  import Header from './Header.svelte';
  import StatusBar from './StatusBar.svelte';

  let { page, content = null }: { page: ClientPage; content?: Element | null } = $props();

  const view = $derived(page.page);
  const list = $derived(view.view === 'list');
  // Browse and error pages are static markup, adopted whole.
  const adopted = $derived(view.view === 'static');
</script>

<StatusBar />
<Header kind={page.kind} base={page.base} site={page.site} icon={shown.icon ?? undefined} />
<main id="content" class="main{list ? ' h-feed' : ''}" tabindex="-1" {@attach adopted ? adopt(content) : null}>
{#if list}<data class="p-name" value={page.title} hidden></data>{/if}
{#key page.path}
{#if view.view === 'list'}
<Feed {page} list={view.data} />
{:else if view.view === 'article'}
<Article {page} article={view.data} {content} />
{:else if view.view === 'preferences'}
<Preferences {page} view={view.data} />
{:else if view.view === 'offline'}
<Offline {page} />
{/if}
{/key}
</main>
<footer class="footer" data-nosnippet><a href="https://github.com/aymericbeaumet/aggr" target="_blank" rel="noopener noreferrer">built with aggr</a></footer>
