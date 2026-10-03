import { describe, expect, it } from 'vitest';
import { patchHead } from './head';

const build = (head: string) => new DOMParser().parseFromString(`<!doctype html><html><head>${head}</head><body></body></html>`, 'text/html');

describe('patchHead', () => {
  it('swaps page metadata and keeps the shared shell', () => {
    const current = build(
      '<title>Feed</title><meta charset="utf-8"><link rel="stylesheet" href="s.css"><script id="aggr-page" type="application/json">{}</script><link rel="canonical" href="https://x/"><meta name="description" content="feed"><link rel="next" href="page/2/"><script type="application/ld+json">{"a":1}</script>',
    );
    const next = build(
      '<title>Article</title><meta charset="utf-8"><link rel="stylesheet" href="s.css"><script id="aggr-page" type="application/json">{"other":1}</script><link rel="canonical" href="https://x/a/"><meta name="description" content="feed"><meta property="og:title" content="A"><script type="application/ld+json">{"b":2}</script>',
    );
    patchHead(current, next);
    expect(current.title).toBe('Article');
    expect(current.querySelector('link[rel=canonical]')?.getAttribute('href')).toBe('https://x/a/');
    expect(current.querySelector('link[rel=next]')).toBeNull();
    expect(current.querySelector('meta[property="og:title"]')?.getAttribute('content')).toBe('A');
    expect(current.querySelectorAll('meta[name=description]')).toHaveLength(1);
    expect(current.querySelector('script[type="application/ld+json"]')?.textContent).toBe('{"b":2}');
    expect(current.querySelector('#aggr-page')?.textContent).toBe('{"other":1}');
    expect(current.querySelectorAll('link[rel=stylesheet]')).toHaveLength(1);
  });
});
