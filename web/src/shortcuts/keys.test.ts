import { describe, expect, it } from 'vitest';
import { externalTarget, gotoTarget, scrollDistance, type Subject } from './keys';

const networks = [
  { name: 'hackernews', url: 'https://hn.algolia.com/?q={url}', shortcut: 'H', found: false },
  { name: 'reddit', url: 'https://www.reddit.com/search/?q={title}', shortcut: 'R', found: false },
];

const subject: Subject = {
  original: 'https://blog.example/a?x=1',
  link: 'https://blog.example/a?x=1',
  title: 'Hello & world',
  discussions: [{ name: 'hackernews', href: 'https://news.ycombinator.com/item?id=1' }],
};

describe('externalTarget', () => {
  it('opens the original, a matched discussion, or the network search', () => {
    expect(externalTarget('O', subject, networks)).toBe('https://blog.example/a?x=1');
    expect(externalTarget('H', subject, networks)).toBe('https://news.ycombinator.com/item?id=1');
    expect(externalTarget('R', subject, networks)).toBe('https://www.reddit.com/search/?q=Hello%20%26%20world');
    expect(externalTarget('X', subject, networks)).toBeNull();
    expect(externalTarget('O', null, networks)).toBeNull();
  });
});

describe('gotoTarget', () => {
  const root = 'https://x.test/reads/';
  it('maps the chord', () => {
    expect(gotoTarget('g', root, [])).toEqual({ top: true });
    expect(gotoTarget('f', root, [])).toEqual({ url: root });
    expect(gotoTarget('i', root, [])).toEqual({ url: root });
    expect(gotoTarget('b', root, [])).toEqual({ url: `${root}browse/` });
    expect(gotoTarget('l', root, [])).toEqual({ url: `${root}browse/` });
    expect(gotoTarget('p', root, [])).toEqual({ url: `${root}preferences/` });
    expect(gotoTarget('2', root, ['items/a/', 'items/b/'])).toEqual({ url: `${root}items/b/` });
    expect(gotoTarget('3', root, ['items/a/'])).toBeNull();
    expect(gotoTarget('0', root, [])).toBeNull();
    expect(gotoTarget('z', root, [])).toBeNull();
    expect(gotoTarget('constructor', root, [])).toBeNull();
  });
});

describe('scrollDistance', () => {
  it('steps by a line or by the preference, capped at half the viewport', () => {
    expect(scrollDistance(24, 800, 10, true)).toBe(24);
    expect(scrollDistance(24, 800, 10, false)).toBe(240);
    expect(scrollDistance(24, 300, 10, false)).toBe(150);
    expect(scrollDistance(24, 10, 1, false)).toBe(12);
  });
});
