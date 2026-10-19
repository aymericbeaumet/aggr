/// <reference types="node" />
import { load } from '../../test/fixtures';
import { afterEach, expect, it, vi } from 'vitest';
import type { ClientPage } from '@aggr-model/ClientPage';
import { badges, lastSeenKey } from '../updates/badges';
import { page } from '../state/page.svelte';
import { boot, mountApp } from './boot';
import { flushSync } from 'svelte';
import { versions } from '../state/versions.svelte';
import { pageArrived } from './reader';

vi.mock('./reader', () => ({
  startReader: () => badges.configure(page.root, sessionStorage, document),
  pageArrived: vi.fn(() => {
    versions.latest = String(Number(versions.latest || 0) + 1);
    const model = page.model;
    badges.arrive([...(model?.site.entries ?? []), ...(model?.page.view === 'list' ? model.page.data.rows.map((row) => row.url) : [])]);
  }),
  refreshToUpdate() {},
  retryConnection() {},
}));
vi.mock('../navigation', () => ({
  navigation: {
    install() {},
    speculate() {},
    replaceAddress() {},
    prefetch() {},
  },
}));

afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); });

it('paints acknowledged new entries before advertising that the client is ready', () => {
  vi.useFakeTimers();
  sessionStorage.clear();
  const fixture = { props: { page: load('page-river') } };
  const model = fixture.props.page;
  if (model.page.view !== 'list') throw new Error('not a list');
  const root = new URL(model.base, location.href);
  sessionStorage.setItem(lastSeenKey(root.pathname), new URL(model.page.data.rows[1].url, root).href);
  document.head.innerHTML = '';
  const script = document.createElement('script');
  script.id = 'aggr-page';
  script.type = 'application/json';
  script.textContent = JSON.stringify(model);
  document.head.append(script);
  load('page-river');
  document.documentElement.removeAttribute('data-aggr-ready');
  boot();
  expect(document.documentElement.dataset.aggrReady).toBe('true');
  expect(document.querySelector('[data-static-feed] .row')?.classList.contains('is-new')).toBe(true);
});


it('settles each navigation once when arrival reads and writes reactive reader state', () => {
  vi.useFakeTimers();
  load('page-river');
  vi.mocked(pageArrived).mockClear();
  const shown = mountApp(document);
  if (!shown) throw new Error('missing page');
  expect(pageArrived).toHaveBeenCalledTimes(1);
  page.show({ ...shown, model: { ...shown.model, path: 'page/2/' }, href: new URL('page/2/', page.root).href });
  flushSync();
  expect(pageArrived).toHaveBeenCalledTimes(2);
  flushSync();
  expect(pageArrived).toHaveBeenCalledTimes(2);
});
