/// <reference types="node" />
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { flushSync } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mountApp } from '../app/boot';
import type { ClientPage } from '../generated/ClientPage';
import { row } from '../search/fixture';
import { fresh } from '../state/fresh.svelte';
import { page } from '../state/page.svelte';
import { badges } from './badges';
import { FeedUpdater, applyFreshList } from './feed';

const directory = resolve(process.cwd(), 'test/parity');

function load(name: string): ClientPage {
  const fixture = JSON.parse(readFileSync(resolve(directory, `${name}.json`), 'utf8')) as { props: { page: ClientPage } };
  document.head.innerHTML = '';
  const script = document.createElement('script');
  script.id = 'aggr-page';
  script.type = 'application/json';
  script.textContent = JSON.stringify(fixture.props.page);
  document.head.append(script);
  document.body.innerHTML = `<div id="app">${readFileSync(resolve(directory, `${name}.html`), 'utf8')}</div>`;
  return fixture.props.page;
}

/** The river fixture as a newer build would render it: one more row on top. */
function newer(model: ClientPage): ClientPage {
  if (model.page.view !== 'list') throw new Error('not a list');
  const arrival = { ...row, path: 'items/blog/2026/09/arrival', url: 'items/blog/2026/09/arrival/', title: 'Just arrived' };
  return {
    ...model,
    build: { ...model.build, content: 'content-next' },
    site: { ...model.site, entries: [arrival.url, ...model.site.entries] },
    page: { view: 'list', data: { ...model.page.data, rows: [arrival, ...model.page.data.rows] } },
  };
}

const FEED = 'ol.rows:not(.search-results)';
const rows = () => [...document.querySelectorAll<HTMLElement>(`${FEED} .row`)];
const scrolledTo = (top: number) => Object.defineProperty(window, 'scrollY', { value: top, configurable: true, writable: true });

describe('the live feed', () => {
  beforeEach(() => {
    sessionStorage.clear();
    fresh.pending = [];
    fresh.marked = [];
    fresh.visible = true;
    scrolledTo(0);
    window.scrollBy = vi.fn();
  });

  it('replaces the rows in the page state and keeps the nodes of the rows already shown', () => {
    const model = load('page-river');
    mountApp(document);
    const before = rows();
    const header = document.querySelector('header.top');
    const main = document.querySelector('main#content');
    const list = document.querySelector(FEED);
    expect(before.length).toBeGreaterThan(0);
    const next = applyFreshList(newer(model));
    expect(next?.build.content).toBe('content-next');
    expect(page.model?.build.content).toBe('content-next');
    expect(page.model?.site.entries[0]).toBe('items/blog/2026/09/arrival/');
    const after = rows();
    expect(after).toHaveLength(before.length + 1);
    expect(after[0].dataset.path).toBe('items/blog/2026/09/arrival');
    expect(after[0].querySelector('.title')?.textContent).toBe('Just arrived');
    // Untouched rows are the same elements; the chrome around the list never moved.
    expect(after.slice(1)).toEqual(before);
    expect(document.querySelector('header.top')).toBe(header);
    expect(document.querySelector('main#content')).toBe(main);
    expect(document.querySelector(FEED)).toBe(list);
    expect(after[0].closest('ol')).toBe(list);
  });

  it('has nothing to swap on a page that is not a list', () => {
    const model = load('page-river');
    mountApp(document);
    expect(applyFreshList({ ...model, page: { view: 'static' } })).toBeNull();
  });

  it('lights the pending entries that are on screen', async () => {
    load('page-river');
    mountApp(document);
    badges.configure(page.root, sessionStorage, document);
    const first = rows()[0];
    const url = new URL(first.dataset.url as string, page.root).href;
    expect(first.classList.contains('is-new')).toBe(false);
    fresh.pending = [url, `${page.root}items/elsewhere/`];
    flushSync();
    expect(rows()[0]).toBe(first);
    expect(first.classList.contains('is-new')).toBe(true);
    expect(fresh.marked).toEqual([url]);
    expect(fresh.pending).toEqual([`${page.root}items/elsewhere/`]);
    expect(sessionStorage.getItem('aggr:new-entries:%2F')).toBe(JSON.stringify([`${page.root}items/elsewhere/`]));
  });

  it('waits for the top of the list unless the reader just came back', async () => {
    const model = load('page-river');
    mountApp(document);
    const loads: string[] = [];
    const updater = new FeedUpdater({
      load: async (href) => {
        loads.push(href);
        return newer(model);
      },
    });
    updater.install(document, window);
    scrolledTo(400);
    updater.notice('content-next');
    await Promise.resolve();
    expect(loads).toHaveLength(0);
    expect(page.model?.build.content).toBe(model.build.content);
    // Back to the window: the swap may go ahead where they left off.
    document.dispatchEvent(new Event('visibilitychange'));
    await vi.waitFor(() => expect(page.model?.build.content).toBe('content-next'));
    expect(loads).toEqual([page.href]);
    expect(rows()[0].dataset.path).toBe('items/blog/2026/09/arrival');
  });

  it('swaps at once at the top of the list, and not for a version the page already shows', async () => {
    const model = load('page-river');
    mountApp(document);
    const load_ = vi.fn(async () => newer(model));
    const updater = new FeedUpdater({ load: load_ });
    updater.install(document, window);
    updater.notice(model.build.content);
    await Promise.resolve();
    expect(load_).not.toHaveBeenCalled();
    updater.notice('content-next');
    await vi.waitFor(() => expect(page.model?.build.content).toBe('content-next'));
    expect(load_).toHaveBeenCalledTimes(1);
  });
});
