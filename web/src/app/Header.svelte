<script lang="ts">
  // `base.html`'s `<header class="top">` and `<nav class="mobile-tabs">`: the persistent
  // navigation, whose links and `aria-current` follow the page kind.
  import type { ClientSite } from '../generated/ClientSite';
  import { urlFor } from '../model/urls';

  let {
    kind,
    base,
    site,
    icon = undefined,
  }: {
    kind: string;
    base: string;
    site: ClientSite;
    /** The brand icon's resolved address; the logical asset name stands in for a server render. */
    icon?: string;
  } = $props();

  // The three destinations, with the page kinds each one is current for and its tab icon.
  const tabs = [
    { route: '', label: 'feed', kinds: ['river'], icon: 'M4 5h16M4 12h16M4 19h10' },
    {
      route: 'browse/',
      label: 'browse',
      kinds: ['browse', 'categories', 'category', 'sources', 'source', 'tags', 'tag'],
      icon: 'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z',
    },
    {
      route: 'preferences/',
      label: 'preferences',
      kinds: ['preferences'],
      icon: 'M4 7h5m4 0h7M4 17h9m4 0h3M9 4v6M13 14v6',
    },
  ] as const;
  const [feed, browse, preferences] = tabs;

  const current = (tab: (typeof tabs)[number]) =>
    (tab.kinds as readonly string[]).includes(kind) ? 'page' : undefined;
</script>

<header class="top" data-nosnippet>
  <nav class="nav" data-site-navigation aria-label="Site navigation">
    <a class="brand" data-route="" rel="home" href={urlFor(base, '')}><img class="brand-icon" src={icon ?? urlFor(base, 'assets/favicon-32.png')} alt="" width="20" height="20"><span class="brand-title">{site.title.toLowerCase()}</span></a>
    <div class="nav-primary">
      <a class="menu-link" data-route={feed.route} data-kinds={feed.kinds.join(' ')} data-feed-action href={urlFor(base, feed.route)} aria-current={current(feed)}>{feed.label}</a>
      <span class="nav-separator" aria-hidden="true">|</span>
      <a class="menu-link" data-route={browse.route} data-kinds={browse.kinds.join(' ')} href={urlFor(base, browse.route)} aria-current={current(browse)}>{browse.label}</a>
      <span class="nav-separator" aria-hidden="true">|</span>
      <a class="menu-link" data-route={preferences.route} data-kinds={preferences.kinds.join(' ')} href={urlFor(base, preferences.route)} aria-current={current(preferences)}>{preferences.label}</a>
    </div>
    <div class="nav-actions">
      <a class="config-link" data-route={site.config_url ? undefined : 'aggr.toml'} href={site.config_url ?? urlFor(base, 'aggr.toml')} target="_blank" rel="noopener noreferrer">aggr.toml <span aria-hidden="true">↗</span></a>
    </div>
  </nav>
</header>
<nav class="mobile-tabs" data-site-navigation aria-label="Main navigation" data-nosnippet>
  {#each tabs as tab, index (tab.route)}{#if index}{' '}{/if}<a data-route={tab.route} data-kinds={tab.kinds.join(' ')} data-feed-action={tab.label === 'feed' ? '' : undefined} href={urlFor(base, tab.route)} aria-current={current(tab)}>
    <svg viewBox="0 0 24 24" width="22" height="22" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d={tab.icon}/></svg>
    <span>{tab.label}</span>
  </a>{/each}
</nav>
