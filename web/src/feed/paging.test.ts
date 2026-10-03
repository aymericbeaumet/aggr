import { describe, expect, it } from 'vitest';
import type { PaginatorCtx } from '../generated/PaginatorCtx';
import { feedPageUrl, sliceFeed } from './paging';

const root = 'https://x.test/reads/';

const paginator = (over: Partial<PaginatorCtx> = {}): PaginatorCtx => ({
  paginate_by: 50,
  base_url: 'page/',
  number_pagers: 3,
  first: './',
  last: 'page/3/',
  previous: null,
  next: 'page/2/',
  current_index: 1,
  total_items: 120,
  offset: 0,
  ...over,
});

describe('feedPageUrl', () => {
  it('keeps the other parameters and only writes slices past the first', () => {
    expect(feedPageUrl('page/2/', 1, `${root}?q=rust&feed-page=3`, root)).toBe(`${root}page/2/?q=rust`);
    expect(feedPageUrl('page/2/', 2, `${root}?q=rust`, root)).toBe(`${root}page/2/?q=rust&feed-page=2`);
    expect(feedPageUrl('', 1, `${root}page/2/?feed-page=2`, root)).toBe(root);
    expect(feedPageUrl(`${root}page/2/`, 3, `${root}page/2/`, root)).toBe(`${root}page/2/?feed-page=3`);
  });
});

describe('sliceFeed', () => {
  it('shows the whole page when the preference is not smaller than the static size', () => {
    const slicing = sliceFeed(paginator(), 50, 50, root, root);
    expect([slicing.start, slicing.end]).toEqual([0, 50]);
    expect(slicing.status).toBe('page 1 / 3');
    expect(slicing.hidden).toBe(false);
    expect(slicing.first.visible).toBe(false);
    expect(slicing.previous.visible).toBe(false);
    expect(slicing.next).toEqual({ href: `${root}page/2/`, visible: true });
    expect(slicing.last).toEqual({ href: `${root}page/3/`, visible: true });
  });

  it('slices a static page into preferred-size pages', () => {
    const first = sliceFeed(paginator(), 50, 10, root, root);
    expect([first.start, first.end]).toEqual([0, 10]);
    expect(first.status).toBe('page 1 / 12');
    expect(first.next).toEqual({ href: `${root}?feed-page=2`, visible: true });
    expect(first.last).toEqual({ href: `${root}page/3/?feed-page=2`, visible: true });

    const third = sliceFeed(paginator(), 50, 10, `${root}?feed-page=3`, root);
    expect([third.start, third.end]).toEqual([20, 30]);
    expect(third.status).toBe('page 3 / 12');
    expect(third.previous).toEqual({ href: `${root}?feed-page=2`, visible: true });
    expect(third.first).toEqual({ href: root, visible: true });

    const edge = sliceFeed(paginator(), 50, 10, `${root}?feed-page=5`, root);
    expect(edge.next).toEqual({ href: `${root}page/2/`, visible: true });
  });

  it('crosses static pages backwards onto the last slice of the previous one', () => {
    const second = sliceFeed(
      paginator({ current_index: 2, previous: './', next: 'page/3/', offset: 50 }),
      50,
      25,
      `${root}page/2/`,
      root,
    );
    expect(second.status).toBe('page 3 / 5');
    expect(second.previous).toEqual({ href: `${root}?feed-page=2`, visible: true });
    expect(second.first).toEqual({ href: root, visible: true });
  });

  it('clamps a slice past the end and hides a pager with one page', () => {
    const last = sliceFeed(
      paginator({ current_index: 3, previous: 'page/2/', next: null, offset: 100 }),
      20,
      10,
      `${root}page/3/?feed-page=9`,
      root,
    );
    expect([last.start, last.end]).toEqual([10, 20]);
    expect(last.status).toBe('page 12 / 12');
    expect(last.next.visible).toBe(false);
    expect(last.last.visible).toBe(false);

    const single = sliceFeed(paginator({ number_pagers: 1, next: null, last: './', total_items: 7 }), 7, 50, root, root);
    expect(single.hidden).toBe(true);
    expect(single.status).toBe('page 1 / 1');
  });
});
