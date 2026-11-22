import { expect, it } from '../../test/browser';
import { fixture, render } from '../../test/fixtures';
import { flushSync, tick } from 'svelte';
import { mountApp } from './boot';
import { page } from '../state/page.svelte';

it('keeps the shared stylesheet attached while navigating between page depths', async () => {
  const initial = fixture('page-river');
  initial.site.assets.css = 'assets/app/test.css';
  const doc = new DOMParser().parseFromString(render({ page: initial, body_html: '' }), 'text/html');
  document.head.innerHTML = doc.head.innerHTML;
  document.body.innerHTML = doc.body.innerHTML;
  const link = document.querySelector<HTMLLinkElement>('link[rel=stylesheet]');
  expect(link).not.toBeNull();
  const href = link?.getAttribute('href');
  const mutations: MutationRecord[] = [];
  const changes = new MutationObserver((records) => mutations.push(...records));
  changes.observe(link!, { attributes: true, attributeFilter: ['href'] });
  mountApp(document);
  await tick();
  expect(document.querySelector('link[rel=stylesheet]')).toBe(link);
  expect(link?.getAttribute('href')).toBe(href);
  expect(mutations).toHaveLength(0);
  expect(changes.takeRecords()).toHaveLength(0);
  for (const name of ['page-browse', 'page-article', 'page-river']) {
    const next = fixture(name);
    next.site.assets.css = initial.site.assets.css;
    const address = new URL(next.path, page.root).href;
    history.replaceState({}, '', address);
    page.show({ model: next, bodyHtml: '', href: address });
    flushSync();
    expect(document.querySelector('link[rel=stylesheet]')).toBe(link);
    expect(link?.getAttribute('href')).toBe(href);
    expect(mutations).toHaveLength(0);
    expect(changes.takeRecords()).toHaveLength(0);
  }
  changes.disconnect();
});
