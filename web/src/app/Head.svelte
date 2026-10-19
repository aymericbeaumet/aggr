<script lang="ts">
  import type { HTMLMetaAttributes } from 'svelte/elements';
  import type { ClientPage } from '@aggr-model/ClientPage';
  import { urlFor } from '../model/urls';
  import { jsonScript } from '../model/json';
  let { page }: { page: ClientPage } = $props();
</script>

<svelte:head>
  <title>{page.head.title}</title>
  {#each page.head.meta as meta}<meta name={meta.name ?? undefined} property={meta.property ?? undefined} http-equiv={(meta.http_equiv ?? undefined) as HTMLMetaAttributes['http-equiv']} id={meta.id ?? undefined} content={meta.content} />{/each}
  {#each page.head.links as link}<link rel={link.rel} href={link.href} type={link.mime_type ?? undefined} title={link.title ?? undefined} sizes={link.sizes ?? undefined} />{/each}
  {#each page.site.assets.imports as path}<link rel="modulepreload" href={urlFor(page.base, path)} />{/each}
  {#if page.site.assets.css}<link rel="stylesheet" href={urlFor(page.base, page.site.assets.css)} />{/if}
  {@html jsonScript('aggr-preferences', page.site.preference_rules)}
  <script src={page.head.bootstrap}></script>
  <noscript><style>.progressive-image,.preview-image,.article-lead img{color:inherit}</style></noscript>
  {@html jsonScript('aggr-page', page)}
  {#if page.head.schema}{@html jsonScript(null, page.head.schema, 'application/ld+json')}{/if}
</svelte:head>
