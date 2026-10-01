import { describe, expect, it } from 'vitest';
import { acknowledge, badgedTitle, detect, lastSeenKey, newEntriesKey, readPending, siteEntries } from './badges';

const root = 'https://reader.test/reader/';
const a = `${root}items/a/`;
const b = `${root}items/b/`;
const c = `${root}items/c/`;

describe('siteEntries', () => {
  it('resolves site paths under the root and drops the rest', () => {
    expect(siteEntries(['items/a/', '/reader/items/b/', 'https://elsewhere.test/x', 'http://[bad', ''], root)).toEqual([a, b, root]);
  });
});

describe('detect', () => {
  it('finds nothing new without a remembered head', () => {
    expect(detect([a, b], null, [])).toEqual([]);
  });

  it('finds nothing new when the head is where it was', () => {
    expect(detect([a, b], a, [c])).toEqual([c]);
  });

  it('takes everything above the remembered head, keeping what was pending', () => {
    expect(detect([a, b, c], c, [])).toEqual([a, b]);
    expect(detect([a, b, c], b, [a])).toEqual([a]);
  });

  it('takes the whole list when the head is gone', () => {
    expect(detect([a, b], c, [])).toEqual([a, b]);
  });
});

describe('acknowledge', () => {
  it('marks the pending entries on screen and keeps the others pending', () => {
    expect(acknowledge([a, b, c], [b, `${root}items/d/`])).toEqual({ marked: [b], pending: [a, c] });
    expect(acknowledge([], [a])).toEqual({ marked: [], pending: [] });
  });
});

describe('badgedTitle', () => {
  it('adds the dot once and takes it away', () => {
    expect(badgedTitle('reader', true)).toBe('● reader');
    expect(badgedTitle('● reader', true)).toBe('● reader');
    expect(badgedTitle('● reader', false)).toBe('reader');
    expect(badgedTitle('reader', false)).toBe('reader');
  });
});

describe('the session keys', () => {
  it('are per site, as the browser suite writes them', () => {
    expect(lastSeenKey('/reader/')).toBe('aggr:last-seen-entry:%2Freader%2F');
    expect(newEntriesKey('/')).toBe('aggr:new-entries:%2F');
  });

  it('read the pending list back leniently', () => {
    const store = new Map<string, string>();
    const storage = { getItem: (key: string) => store.get(key) ?? null, setItem: (key: string, value: string) => void store.set(key, value) };
    expect(readPending(storage, 'k')).toEqual([]);
    store.set('k', JSON.stringify([a, 1, b]));
    expect(readPending(storage, 'k')).toEqual([a, b]);
    store.set('k', '{');
    expect(readPending(storage, 'k')).toEqual([]);
    expect(readPending(null, 'k')).toEqual([]);
  });
});
