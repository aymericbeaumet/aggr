import { describe, expect, it } from 'vitest';
import { PageCache, type Fetched } from './cache';

const page = (key: string): Fetched => ({ url: key, html: `<p>${key}</p>` });

describe('PageCache', () => {
  it('shares one request per address and keeps the most recent 24', async () => {
    let requests = 0;
    const cache = new PageCache(async (key) => {
      requests += 1;
      return page(key);
    });
    const first = cache.load('a');
    expect(cache.load('a')).toBe(first);
    expect(requests).toBe(1);
    await first.page;
    expect(first.ready).toBe(true);
    expect(first.fetched).toEqual(page('a'));
    for (let index = 0; index < 30; index += 1) cache.load(`p${index}`);
    expect(cache.size).toBe(24);
    expect(cache.has('a')).toBe(false);
    expect(cache.has('p6')).toBe(true);
    expect(cache.has('p5')).toBe(false);
  });

  it('moves a hit to the back of the queue', () => {
    const cache = new PageCache(async (key) => page(key), { limit: 2 });
    cache.load('a');
    cache.load('b');
    cache.load('a');
    cache.load('c');
    expect(cache.keys()).toEqual(['a', 'c']);
  });

  it('fetches a stale page again and keeps the old copy for a failed refresh', async () => {
    let now = 0;
    let fail = false;
    let requests = 0;
    const cache = new PageCache(
      async (key) => {
        requests += 1;
        if (fail) throw new Error('offline');
        return page(key);
      },
      { lifetime: 1000, now: () => now },
    );
    const first = cache.load('a');
    await first.page;
    now = 999;
    expect(cache.load('a')).toBe(first);
    now = 1000;
    fail = true;
    const second = cache.load('a');
    expect(second).not.toBe(first);
    expect(await second.page).toEqual(page('a'));
    expect(second.ready).toBe(true);
    expect(cache.peek('a')).toBe(second);
    expect(requests).toBe(2);
  });

  it('forgets a request that failed with nothing to fall back on', async () => {
    const cache = new PageCache(async () => {
      throw new Error('404');
    });
    const record = cache.load('a');
    await expect(record.page).rejects.toThrow('404');
    await Promise.resolve();
    expect(cache.has('a')).toBe(false);
  });

  it('announces arrivals', async () => {
    const ready: string[] = [];
    const cache = new PageCache(async (key) => page(key), { onReady: (key) => ready.push(key) });
    await cache.load('a').page;
    expect(ready).toEqual(['a']);
  });
});
