import { untrack } from 'svelte';
import { sitePath } from '../model/urls';
import { cursorRow } from '../selection/cursor';
import { page } from '../state/page.svelte';
import { preferences } from '../state/preferences.svelte';
import { search } from '../state/search.svelte';
import { selection } from '../state/selection.svelte';
import { fresh } from '../state/fresh.svelte';
import { badges } from '../updates/badges';
import { sliceFeed } from './paging';
import { installToolbar } from './toolbar.svelte';

/** Bind state to the list Rust rendered, keeping its rows and template-specific markup. */
export function installFeed(root: HTMLElement): () => void {
  const stopToolbar = installToolbar(root);
  const stop = $effect.root(() => {
    $effect(() => {
      const model = page.model;
      const href = page.href;
      if (!model || model.page.view !== 'list' || !href) return;
      const list = model.page.data;
      const scoped = list.scope?.kind === model.kind ? list.scope : null;
      untrack(() => search.arrive({ root: page.root, href, seed: scoped ? `${model.kind}:${scoped.slug} ` : '', scope: scoped ? { kind: model.kind, slug: scoped.slug } : null }));
    });
    $effect(() => {
      const model = page.model;
      const href = page.href;
      if (!model || model.page.view !== 'list' || !href) return;
      const list = model.page.data;
      const slicing = model.kind === 'river' ? sliceFeed(list.paginator, list.rows.length, preferences.feedPageSize, href, page.root) : null;
      const searching = search.attached && search.active;
      const feed = root.querySelector<HTMLElement>('[data-static-feed]');
      if (feed) feed.hidden = searching;
      document.body.toggleAttribute('data-searching', searching);
      const rows = [...root.querySelectorAll<HTMLElement>('[data-static-feed] .row')];
      for (const [index, row] of rows.entries()) row.hidden = slicing !== null && (index < slicing.start || index >= slicing.end);
      if (!searching) {
        const visible = slicing ? list.rows.slice(slicing.start, slicing.end) : list.rows;
        untrack(() => selection.sync(visible.map((row) => cursorRow(row, page.root)), href));
      }
      const pager = root.querySelector<HTMLElement>('[data-feed-pager]');
      if (pager && slicing) {
        pager.hidden = slicing.hidden;
        for (const name of ['first', 'previous', 'next', 'last'] as const) {
          const link = pager.querySelector<HTMLAnchorElement>(`[data-page-${name}]`);
          if (link) { link.href = slicing[name].href; link.hidden = !slicing[name].visible; }
        }
        const status = pager.querySelector('[data-page-status]');
        if (status) status.textContent = slicing.status;
      }
    });
    $effect(() => {
      void page.model;
      const current = selection.url;
      const searching = search.attached && search.active;
      const marked = fresh.marked;
      for (const row of root.querySelectorAll<HTMLElement>('[data-static-feed] .row[data-url]')) {
        const href = new URL(sitePath(row.dataset.url || ''), page.root).href;
        row.classList.toggle('is-selected', !searching && href === current);
        row.classList.toggle('is-new', marked.includes(href));
      }
    });
    $effect(() => {
      const model = page.model;
      if (!model || model.page.view !== 'list' || search.active || !fresh.visible || !fresh.pending.length) return;
      const urls = model.page.data.rows.map((row) => new URL(sitePath(row.url), page.root).href);
      untrack(() => badges.reveal(urls));
    });
  });
  return () => { stop(); stopToolbar(); search.leave(); document.body.removeAttribute('data-searching'); };
}
