/// <reference types="node" />
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, expect, it, vi } from 'vitest';
import type { ClientPage } from '../generated/ClientPage';
import { badges, lastSeenKey } from '../updates/badges';
import { page } from '../state/page.svelte';
import { boot } from './boot';

vi.mock('./reader', () => ({
  startReader: () => badges.configure(page.root, sessionStorage, document),
  pageArrived: () => {
    const model = page.model;
    badges.arrive([...(model?.site.entries ?? []), ...(model?.page.view === 'list' ? model.page.data.rows.map((row) => row.url) : [])]);
  },
  refreshToUpdate() {},
  retryConnection() {},
}));
vi.mock('../navigation', () => ({
  navigation: {
    install: (hooks: Array<(signal: AbortSignal) => void>) => hooks.forEach((hook) => hook(new AbortController().signal)),
    speculate() {},
    replaceAddress() {},
    prefetch() {},
  },
}));

afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); });

it('paints acknowledged new entries before advertising that the client is ready', () => {
  vi.useFakeTimers();
  sessionStorage.clear();
  const fixture = JSON.parse(readFileSync(resolve(process.cwd(), 'test/parity/page-river.json'), 'utf8')) as { props: { page: ClientPage } };
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
  document.body.innerHTML = `<div id="app">${readFileSync(resolve(process.cwd(), 'test/parity/page-river.html'), 'utf8')}</div>`;
  document.documentElement.removeAttribute('data-aggr-ready');
  boot();
  expect(document.documentElement.dataset.aggrReady).toBe('true');
  expect(document.querySelector('[data-static-feed] .row')?.classList.contains('is-new')).toBe(true);
});
