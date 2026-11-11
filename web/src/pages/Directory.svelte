<script lang="ts">
  import type { ClientPage } from '@aggr-model/ClientPage';
  import type { ClientDirectory } from '@aggr-model/ClientDirectory';
  import { urlFor, facetUrl } from '../model/urls';
  import { formatDate } from '../dates/format';
  let { page, directory }: { page: ClientPage; directory: ClientDirectory } = $props();
  const intros: Record<string, string> = {
    categories: 'Categories group sources by subject. Choose one to see all matching articles in the feed.',
    sources: 'Sources are the feeds and sites you follow. Choose one to see its articles in the feed.',
    tags: 'Tags describe topics attached to articles. Choose one to see matching articles across sources.',
  };
</script>
<section class="page browse-page" aria-labelledby="page-title">
  <header class="page-header"><h1 id="page-title">{page.title}</h1><p class="muted browse-intro">{intros[page.kind] ?? 'Browse by category, source, or tag to filter the main feed.'}</p>
  {#if page.kind === 'browse' || page.kind === 'sources'}<nav class="reading-list-actions" aria-label="Use this reading list"><a href={urlFor(page.base, 'sources.opml')} download>Download subscriptions (OPML)</a><a href={urlFor(page.base, 'rss.xml')} rel="alternate" type="application/rss+xml">Subscribe to this reader</a><a href={page.site.config_url ?? urlFor(page.base, 'aggr.toml')} target="_blank" rel="noopener noreferrer">View reading list</a></nav><p class="muted">To share a filtered view, choose a source or category and copy its address.</p>{/if}
  </header>
  <div class="page-body browse-directory"><div class="browse-groups">
  {#each directory.groups as group}
    <section class="browse-group browse-group-{group.kind}" id={group.kind} aria-labelledby={page.kind === 'browse' ? `${group.kind}-title` : 'page-title'}>
    {#if page.kind === 'browse'}<h2 id="{group.kind}-title">{group.title}</h2>{/if}
    {#if group.entries.length}<ul class="browse-entries" role="list" aria-label="{group.entries.length} {group.title.toLowerCase()}">
      {#each group.entries as entry}<li class="browse-entry" data-directory-entry>
        <div class="browse-entry-heading"><a class="browse-entry-link" href={urlFor(page.base, entry.url)}>{group.kind === 'tags' ? '#' : group.kind === 'categories' ? '/' : ''}{entry.name}</a>{#if entry.site_url}<a class="browse-entry-website" href={entry.site_url} title={entry.site_url} target="_blank" rel="noopener noreferrer" data-aggr-external-label="Open {entry.name} website" aria-label="Open {entry.name} website, opens in a new tab">↗</a>{/if}</div>
        <p class="browse-entry-meta"><span>{entry.count} {entry.count === 1 ? 'item' : 'items'}</span>{#if entry.latest}<span><span class="sr-only">Latest: </span><time datetime={entry.latest} title={entry.latest}>{formatDate(entry.latest)}</time></span>{/if}{#if entry.engine}<span>{entry.engine}</span>{/if}{#if entry.category}<span><a href={facetUrl(page.base, entry.category.slug, 'category')}>/{entry.category.name}</a></span>{/if}</p>
        {#if entry.error}<p class="browse-entry-error error" title={entry.error.message}>error since <time datetime={entry.error.since} title={entry.error.since}>{formatDate(entry.error.since)}</time></p>{/if}
      </li>{/each}
    </ul>{:else}<p class="empty">0 {group.kind}. {#if group.kind === 'sources'}Add a <code>[[sources]]</code> table with <code>url = "…"</code>{:else if group.kind === 'categories'}Add <code>category = "…"</code> to a source{:else}Add <code>labels = ["…"]</code> to a source{/if} in <code>aggr.toml</code>.</p>{/if}
    </section>
  {/each}
  </div></div>
</section>
