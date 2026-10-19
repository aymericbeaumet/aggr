<script lang="ts">
  import type { ClientPage } from '@aggr-model/ClientPage';
  import Head from './Head.svelte';
  import NowPlaying from './NowPlaying.svelte';
  import Directory from '../pages/Directory.svelte';
  import Shortcuts from './Shortcuts.svelte';
  import { lifecycle } from './lifecycle';
  import Offline from '../offline/Offline.svelte';
  import Article from '../pages/Article.svelte';
  import Feed from '../pages/Feed.svelte';
  import Preferences from '../pages/Preferences.svelte';
  import Header from './Header.svelte';
  import StatusBar from './StatusBar.svelte';

  let { page, bodyHtml = '' }: { page: ClientPage; bodyHtml?: string } = $props();

  const view = $derived(page.page);
  const list = $derived(view.view === 'list');

</script>

<Head {page} />
<a class="skip-link" href="#content">skip to content</a>
<Shortcuts discussions={page.site.discussions} />
<StatusBar />
<div id="now-playing-root"><NowPlaying /></div>
<Header kind={page.kind} base={page.base} site={page.site} icon={page.base + page.site.assets.icon} />
<main id="content" class="main{list ? ' h-feed' : ''}" tabindex="-1">
{#if list}<data class="p-name" value={page.title} hidden></data>{/if}
{#key page.path}
<div class="page-content" {@attach lifecycle(page)}>
{#if view.view === 'list'}
<Feed {page} list={view.data} />
{:else if view.view === 'article'}
<Article {page} article={view.data} {bodyHtml} />
{:else if view.view === 'preferences'}
<Preferences {page} view={view.data} />
{:else if view.view === 'offline'}
<div class="listhead"><h1>Offline</h1></div>
<p>This page has not been saved. Open one of your downloaded articles below, or reconnect and try again.</p>
<div data-offline-root><Offline {page} /></div>
{:else if view.view === 'directory'}
<Directory {page} directory={view.data} />
{:else if view.view === 'not_found'}
<div class="listhead"><h1>Not found</h1></div>
<p class="empty">No page here. Items older than the site window live on in git:
{#if view.data.archive_url}<a href={view.data.archive_url}>browse the <code>{view.data.data_branch}</code> branch</a>{:else}see the <code>{view.data.data_branch}</code> branch{/if}
 or go back to the <a href={page.base}>feed</a>.</p>
{/if}
</div>
{/key}
</main>
<footer class="footer" data-nosnippet><a href="https://github.com/aymericbeaumet/aggr" target="_blank" rel="noopener noreferrer">built with aggr</a></footer>
