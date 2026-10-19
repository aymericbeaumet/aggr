import { flushSync, mount, unmount } from 'svelte';
import type { ClientPage } from '../generated/ClientPage';
import { enhance } from '../article/enhance';
import { swipeable } from '../article/swipes';
import { installFeed } from '../feed/enhance.svelte';
import { installMetadata } from '../feed/metadata.svelte';
import Offline from '../offline/Offline.svelte';
import { installPreferences } from '../preferences/form.svelte';

const installed = new WeakMap<Document, () => void>();

/** End page-owned behavior before removing its nodes, so playing media can be parked. */
export function releasePage(doc: Document = document): void {
  installed.get(doc)?.();
  installed.delete(doc);
}

/** Enhance the HTML Rust authored; the model supplies behavior, never a second page render. */
export function enhancePage(root: HTMLElement, model: ClientPage): void {
  const doc = root.ownerDocument;
  releasePage(doc);
  const cleanups: Array<() => void> = [installMetadata(root)];
  if (model.page.view === 'list') cleanups.push(installFeed(root));
  if (model.page.view === 'article') {
    const article = root.querySelector<HTMLElement>('article.item');
    const body = root.querySelector<HTMLElement>('[data-article-content]');
    if (article) {
      const stop = swipeable(article);
      if (stop) cleanups.push(stop);
    }
    if (body) {
      const stop = enhance({ next: model.page.data.next, previous: model.page.data.previous })(body);
      if (stop) cleanups.push(stop);
    }
  }
  if (model.page.view === 'preferences') {
    const body = root.querySelector<HTMLElement>('.preferences-body');
    if (body) cleanups.push(installPreferences(body, model.page.data.schema));
  }
  if (model.page.view === 'offline') {
    const target = root.querySelector<HTMLElement>('[data-offline-root]');
    if (target) {
      target.replaceChildren();
      const instance = mount(Offline, { target, props: { page: model } });
      cleanups.push(() => { void unmount(instance); });
    }
  }
  let stopped = false;
  installed.set(doc, () => {
    if (stopped) return;
    stopped = true;
    for (const cleanup of cleanups.reverse()) cleanup();
  });
  flushSync();
}
