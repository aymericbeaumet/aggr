import { load, fixture, render } from '../../test/fixtures';
/// <reference types="node" />
import { flushSync } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mountApp } from '../app/boot';
import type { ClientPage } from '@aggr-model/ClientPage';
import { row } from '../search/fixture';
import { fresh } from '../state/fresh.svelte';
import { page } from '../state/page.svelte';
import { selection } from '../state/selection.svelte';
import { cursorRow } from '../selection/cursor';
import { badges } from './badges';
import { FeedUpdater, applyFreshList } from './feed';


/** The river fixture as a newer build would render it: one more row on top. */
function newer(model: ClientPage): ClientPage {
  if (model.page.view !== 'list') throw new Error('not a list');
  const arrival = { ...row, path: 'items/blog/2026/09/arrival', url: 'items/blog/2026/09/arrival/', title: 'Just arrived' };
  const next: ClientPage = {
    ...model,
    build: { ...model.build, content: 'content-next' },
    site: { ...model.site, entries: [arrival.url, ...model.site.entries] },
    page: { view: 'list', data: { ...model.page.data, rows: [arrival, ...model.page.data.rows] } },
  };
  return next;
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
    expect(applyFreshList({ ...model, page: { view: 'not_found', data: { data_branch: 'data', archive_url: null } } })).toBeNull();
  });

  it('updates ranks when a new row shifts retained nodes down the list', () => {
    const model = load('page-river');
    mountApp(document);
    const before = rows();
    applyFreshList(newer(model));
    const after = rows();
    expect(after.slice(1)).toEqual(before);
    expect(after.map((node) => node.querySelector('.rank')?.textContent)).toEqual(
      after.map((_, index) => `${index + 1}.`),
    );
  });

  it('updates a source warning through recovery and failure without replacing search or selection', () => {
    const model = load('page-source-error');
    if (model.page.view !== 'list') throw new Error('not a list');
    mountApp(document);
    selection.configure(new URL(page.root).pathname, sessionStorage);
    const toolbar = document.querySelector('.feed-toolbar');
    const input = document.querySelector<HTMLInputElement>('#q')!;
    input.setSelectionRange(2, 5);
    const selected = rows()[1];
    selection.select(cursorRow(model.page.data.rows[1], page.root), page.href!);
    flushSync();
    selected.querySelector<HTMLAnchorElement>('[data-row-open]')!.focus();
    const warning = document.querySelector('.listhead .error')!.cloneNode(true) as HTMLElement;
    const recovered = structuredClone(model);
    if (recovered.page.view !== 'list') throw new Error('not a list');
    recovered.page.data.error = null;
    recovered.build.content = 'recovered';
    const recoveredContent = document.querySelector('main')!.cloneNode(true) as HTMLElement;
    recoveredContent.querySelector('.listhead .error')!.remove();
    recoveredContent.querySelector<HTMLElement>('.listhead')!.dataset.rustState = 'recovered';
    applyFreshList(recovered);
    expect(document.querySelector('.listhead .error')).toBeNull();
    expect(document.querySelector('.feed-toolbar')).toBe(toolbar);
    expect(document.querySelector('#q')).toBe(input);
    expect([input.selectionStart, input.selectionEnd]).toEqual([2, 5]);
    expect(document.querySelector('.row.is-selected')).toBe(selected);
    expect(document.activeElement).toBe(selected.querySelector('[data-row-open]'));

    const failed = structuredClone(recovered);
    if (failed.page.view !== 'list') throw new Error('not a list');
    failed.page.data.error = { message: 'HTTP 429', since: '2026-10-19T00:00:00Z' };
    failed.build.content = 'failed-again';
    const failedContent = document.querySelector('main')!.cloneNode(true) as HTMLElement;
    warning.title = 'HTTP 429';
    warning.textContent = '⚠ HTTP 429';
    failedContent.querySelector('.listhead .listtools')!.append(warning);
    applyFreshList(failed);
    expect(document.querySelector('.listhead .error')?.textContent).toContain('⚠ HTTP 429');
    expect(document.querySelector('.feed-toolbar')).toBe(toolbar);
    expect(document.querySelector('#q')).toBe(input);
    expect([input.selectionStart, input.selectionEnd]).toEqual([2, 5]);
    expect(document.querySelector('.row.is-selected')).toBe(selected);
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
