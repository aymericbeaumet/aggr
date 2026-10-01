/// <reference types="node" />
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { flushSync } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mountApp } from '../app/boot';
import type { ClientRow } from '../generated/ClientRow';
import { search } from '../state/search.svelte';
import { selection } from '../state/selection.svelte';
import { hex, row } from './fixture';
import type { Index, SearchPage, Session } from './engine';
import type { Catalog } from './types';

const fake = vi.hoisted(() => ({
  results: [] as { url: string; title: string }[],
  runs: [] as string[],
}));

const catalogue: Catalog = { version: 'v1', base: 'pagefind/v1/', docs: 3, facets: { source: [{ value: 'blog.example', label: 'Blog', count: 3 }] } };

vi.mock('./engine', () => {
  const index: Index = {
    manifest: catalogue,
    retire() {},
    async prepare() {},
    async run(query): Promise<SearchPage> {
      fake.runs.push(query.raw);
      const results = fake.results.map(({ url, title }) => ({
        url,
        meta: { title, aggr_display: hex({ ...row, url, path: url.replace(/\/$/, ''), title } satisfies ClientRow) },
      }));
      return { results, total: results.length, page: 1, pages: 1, size: 50 };
    },
    async counts() {
      return [];
    },
  };
  const session: Session = { load: async () => index, stale() {}, network: () => false, dispose() {} };
  return { createSession: () => session, isOnline: () => true };
});

const directory = resolve(process.cwd(), 'test/parity');

function load(name: string): void {
  const fixture = JSON.parse(readFileSync(resolve(directory, `${name}.json`), 'utf8')) as { props: { page: unknown } };
  document.head.innerHTML = '';
  const script = document.createElement('script');
  script.id = 'aggr-page';
  script.type = 'application/json';
  script.textContent = JSON.stringify(fixture.props.page);
  document.head.append(script);
  // The fixture holds what `App` renders inside the root; boot creates the root itself.
  document.body.innerHTML = `<div id="app">${readFileSync(resolve(directory, `${name}.html`), 'utf8')}</div>`;
}

const field = () => document.getElementById('q') as HTMLInputElement;
const feed = () => document.querySelector('[data-static-feed]') as HTMLElement;

function type(text: string): void {
  const input = field();
  input.value = text;
  input.setSelectionRange(text.length, text.length);
  input.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
}

describe('search on the feed', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    sessionStorage.clear();
    history.replaceState(null, '', '/');
    fake.results = [
      { url: 'items/blog/2026/09/one/', title: 'One' },
      { url: 'items/blog/2026/09/two/', title: 'Two' },
    ];
    fake.runs = [];
    load('page-river');
    mountApp(document);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('hides the static feed as soon as a query is typed and shows the results in its place', async () => {
    expect(feed().hidden).toBe(false);
    type('rust');
    // Before the module has even loaded: the state alone hides the feed.
    expect(feed().hidden).toBe(true);
    expect(document.body.hasAttribute('data-searching')).toBe(true);
    await search.load();
    await vi.advanceTimersByTimeAsync(100);
    flushSync();
    expect(fake.runs).toEqual(['rust']);
    const rows = document.querySelectorAll('#list .row');
    expect(rows).toHaveLength(2);
    expect(rows[0].querySelector('.title')?.textContent).toBe('One');
    expect(rows[0].classList.contains('is-selected')).toBe(true);
    expect(document.getElementById('search-status')?.textContent).toBe('2 articles');
    expect(document.getElementById('list')?.hidden).toBe(false);
    expect(new URL(location.href).searchParams.get('q')).toBe('rust');
    expect(JSON.parse(sessionStorage.getItem(`aggr:search-snapshot:${encodeURIComponent('/')}`) || '{}')).toMatchObject({ query: 'rust', page: 1 });
    // Clearing hands the list and its cursor back to the feed.
    type('');
    await vi.advanceTimersByTimeAsync(100);
    flushSync();
    expect(feed().hidden).toBe(false);
    expect(document.body.hasAttribute('data-searching')).toBe(false);
    expect(document.getElementById('list')?.hidden).toBe(true);
    expect(new URL(location.href).searchParams.has('q')).toBe(false);
    expect(selection.current?.href).toContain('items/blog/');
  });

  it('keeps a focused result by URL while staged results replace the rows around it', async () => {
    type('rust');
    await search.load();
    await vi.advanceTimersByTimeAsync(100);
    flushSync();
    const two = Array.from(document.querySelectorAll<HTMLAnchorElement>('#list [data-row-open]')).find((link) => link.textContent === 'Two');
    expect(two).toBeDefined();
    two?.focus();
    expect(document.activeElement).toBe(two);
    const href = two?.href;
    // The refreshed results arrive in another order, with a newcomer in front.
    fake.results = [
      { url: 'items/blog/2026/09/three/', title: 'Three' },
      { url: 'items/blog/2026/09/two/', title: 'Two' },
      { url: 'items/blog/2026/09/one/', title: 'One' },
    ];
    search.driver?.refresh();
    expect(document.querySelectorAll('#list .row')).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(100);
    flushSync();
    expect(document.querySelectorAll('#list .row')).toHaveLength(3);
    const focused = document.activeElement;
    expect(focused).toBeInstanceOf(HTMLAnchorElement);
    expect((focused as HTMLAnchorElement).href).toBe(href);
    expect(focused?.textContent).toBe('Two');
  });
});
