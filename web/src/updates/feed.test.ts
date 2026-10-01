import { describe, expect, it } from 'vitest';
import type { ClientPage } from '../generated/ClientPage';
import { row } from '../search/fixture';
import { shouldApply, withFreshList } from './feed';

describe('shouldApply', () => {
  it('swaps at the top of the list, or when the reader just came back', () => {
    expect(shouldApply(0, false)).toBe(true);
    expect(shouldApply(-1, false)).toBe(true);
    expect(shouldApply(300, false)).toBe(false);
    expect(shouldApply(300, true)).toBe(true);
  });
});

const list = (content: string, rows: typeof row[], entries: string[]): ClientPage => ({
  base: './',
  path: '',
  kind: 'river',
  title: 'reader',
  site: { title: 'reader', language: 'en', pwa: true, excerpts: true, discussions: [], entries, config_url: null, assets: { app: null, css: null, imports: [] } },
  build: { version: '1.0.0', app: 'app-1', content },
  page: { view: 'list', data: { rows, paginator: { offset: 0, first: '', last: '', previous: null, next: null, page: 1, pages: 1 } as never, scope: null, error: null } },
});

describe('withFreshList', () => {
  it('takes the fetched rows, versions and shortcuts and keeps the page', () => {
    const newer = { ...row, path: 'items/blog/2026/09/newer', url: 'items/blog/2026/09/newer/' };
    const current = list('content-1', [row], ['items/blog/2026/09/plain/']);
    const freshPage = list('content-2', [newer, row], ['items/blog/2026/09/newer/', 'items/blog/2026/09/plain/']);
    const next = withFreshList(current, freshPage);
    expect(next).not.toBeNull();
    expect(next?.build.content).toBe('content-2');
    expect(next?.site.entries).toEqual(freshPage.site.entries);
    expect(next?.page).toBe(freshPage.page);
    expect(next?.kind).toBe('river');
    expect(next?.site.title).toBe(current.site.title);
    // The page on screen is left as it was.
    expect(current.build.content).toBe('content-1');
  });

  it('has nothing to swap unless both sides are lists', () => {
    const current = list('content-1', [row], []);
    const article: ClientPage = { ...current, page: { view: 'static' } };
    expect(withFreshList(article, current)).toBeNull();
    expect(withFreshList(current, article)).toBeNull();
  });
});
