import { describe, expect, it } from 'vitest';
import { cursorKey, cursorRow, readCursor, stepIndex, writeCursor } from './cursor';

class MemoryStore {
  private map = new Map<string, string>();
  getItem(key: string) {
    return this.map.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.map.set(key, value);
  }
}

describe('cursorKey', () => {
  it('scopes the key to the site and the address without its fragment', () => {
    expect(cursorKey('/reads/', 'https://x.test/reads/page/2/?feed-page=2#top')).toBe(
      `aggr:list-cursor:${encodeURIComponent('/reads/')}:${encodeURIComponent('https://x.test/reads/page/2/?feed-page=2')}`,
    );
  });
});

describe('read and write', () => {
  it('round-trips the selected URL and ignores junk', () => {
    const store = new MemoryStore();
    expect(readCursor(store, 'k')).toBeNull();
    expect(writeCursor(store, 'k', 'https://x.test/items/a/')).toBe(true);
    expect(readCursor(store, 'k')).toBe('https://x.test/items/a/');
    store.setItem('k', '{"url":1}');
    expect(readCursor(store, 'k')).toBeNull();
    store.setItem('k', '{');
    expect(readCursor(store, 'k')).toBeNull();
  });

  it('reports a store that refuses', () => {
    const store = {
      getItem: () => null,
      setItem: () => {
        throw new Error('quota');
      },
    };
    expect(writeCursor(store, 'k', 'u')).toBe(false);
  });
});

describe('stepIndex', () => {
  it('starts at the first row and holds at the ends', () => {
    expect(stepIndex(-1, 1, 3)).toBe(0);
    expect(stepIndex(-1, -1, 3)).toBe(0);
    expect(stepIndex(0, -1, 3)).toBe(0);
    expect(stepIndex(2, 1, 3)).toBe(2);
    expect(stepIndex(1, 1, 3)).toBe(2);
    expect(stepIndex(0, 1, 0)).toBe(-1);
  });
});

describe('cursorRow', () => {
  it('resolves the item page against the site root', () => {
    const row = cursorRow(
      {
        path: 'items/blog/2026/09/a',
        url: 'items/blog/a/',
        link: 'https://blog.example/a',
        title: 'A',
        language: null,
        excerpt: '',
        age_band: 'h24',
        category: null,
        labels: [],
        preview: null,
        metadata: {
          original: 'https://blog.example/a',
          date: '2026-09-30T00:00:00+00:00',
          source_slug: 'blog',
          source_query: 'blog',
          source_display: 'blog.example',
          source_title: 'Blog',
          feed_sources: [],
          is_aggregated: false,
          word_count: 0,
          reading_minutes: 0,
          discussions: [{ name: 'hackernews', url: 'https://news.ycombinator.com/item?id=1' }],
        },
      },
      'https://x.test/reads/',
    );
    expect(row.href).toBe('https://x.test/reads/items/blog/a/');
    expect(row.discussions).toEqual([{ name: 'hackernews', href: 'https://news.ycombinator.com/item?id=1' }]);
  });
});
