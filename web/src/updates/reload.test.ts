import { describe, expect, it } from 'vitest';
import { parsePlace, reloadKey } from './reload';

describe('the reload place', () => {
  const href = 'https://reader.test/reader/items/a/';

  it('is keyed per site', () => {
    expect(reloadKey('/reader/')).toBe('aggr:reader-reload:%2Freader%2F');
  });

  it('comes back only for the document it was saved from', () => {
    const saved = JSON.stringify({ url: href, x: 0, y: 300, focus: 'q', help: true });
    expect(parsePlace(saved, href)).toEqual({ url: href, x: 0, y: 300, focus: 'q', help: true });
    expect(parsePlace(saved, 'https://reader.test/reader/')).toBeNull();
    expect(parsePlace(JSON.stringify({ url: href, x: 0, y: 300 }), href)).toEqual({ url: href, x: 0, y: 300, focus: '', help: false });
  });

  it('ignores what it cannot read', () => {
    expect(parsePlace(null, href)).toBeNull();
    expect(parsePlace('', href)).toBeNull();
    expect(parsePlace('{', href)).toBeNull();
    expect(parsePlace(JSON.stringify({ url: href, x: '0', y: 300 }), href)).toBeNull();
    expect(parsePlace('"text"', href)).toBeNull();
  });
});
