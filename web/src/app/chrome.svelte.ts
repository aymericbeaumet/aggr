import { pull } from '../offline/pull-refresh.svelte';
import { announcer } from '../state/announcer.svelte';
import { connection } from '../state/connection.svelte';
import { page } from '../state/page.svelte';
import { tabIntent } from '../state/tabIntent.svelte';
import { versions } from '../state/versions.svelte';
import { refreshToUpdate, retryConnection } from './reader';

/** Keep the Rust-rendered shell and update only its interactive state. */
export function installChrome(): () => void {
  // The shell survives navigation, so its URLs must keep their original document base.
  for (const node of document.querySelectorAll('head link[href], [data-site-navigation] [href], [data-site-navigation] [src]')) {
    // These resources already resolved their URLs; rewriting href restarts their downloads.
    if (node instanceof HTMLLinkElement && ['stylesheet', 'preload', 'modulepreload'].some((rel) => node.relList.contains(rel))) continue;
    for (const attribute of ['href', 'src']) {
      const value = node.getAttribute(attribute);
      if (!value || value.startsWith('#')) continue;
      try { node.setAttribute(attribute, new URL(value, document.baseURI).href); }
      catch { /* Leave malformed template URLs to the browser. */ }
    }
  }
  const announcement = document.getElementById('reader-announcement');
  const indicator = document.getElementById('pull-refresh');
  const banner = document.getElementById('connection-status');
  const message = document.getElementById('connection-status-message');
  const retry = document.getElementById('connection-retry');
  const refresh = document.getElementById('pwa-refresh');
  const tabs = [...document.querySelectorAll<HTMLElement>('[data-site-navigation] [data-kinds]')];
  const controller = new AbortController();
  retry?.addEventListener('click', retryConnection, { signal: controller.signal });
  refresh?.addEventListener('click', refreshToUpdate, { signal: controller.signal });
  const stop = $effect.root(() => {
    $effect(() => {
      const chosen = tabIntent.route;
      for (const tab of tabs) {
        const current = chosen !== null ? tab.dataset.route === chosen : (tab.dataset.kinds ?? '').split(' ').includes(page.kind);
        if (current) tab.setAttribute('aria-current', 'page');
        else tab.removeAttribute('aria-current');
      }
    });
    $effect(() => {
      if (announcement) announcement.textContent = announcer.message;
      if (indicator) { indicator.hidden = pull.hidden; indicator.textContent = pull.label; }
      const update = versions.releasePending;
      const online = connection.online;
      document.documentElement.dataset.updateState = update ? 'ready' : 'current';
      if (banner) banner.hidden = online && !update;
      if (message) {
        message.hidden = update;
        message.textContent = online ? '' : 'Offline — downloaded articles remain available';
      }
      if (retry) retry.hidden = online || update;
      if (refresh) refresh.hidden = !update;
    });
  });
  return () => { controller.abort(); stop(); };
}
