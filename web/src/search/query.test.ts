import { describe, expect, it } from 'vitest';
import { canonicalQuery, dateMatches, parseQuery, queryFromHref, queryURL, quoteValue, stripSearch, tokenize } from './query';

describe('query parsing', () => {
  it('keeps quoted phrases, escaped quotes, facet identifiers and exclusions', () => {
    const query = parseQuery('rust "borrow checker" source:"The \\"Rust\\" Blog" -tag:ads -"old news"');
    expect(query.clauses.map((clause) => [clause.kind, clause.value, clause.exclude])).toEqual([
      ['text', 'rust', false],
      ['text', 'borrow checker', false],
      ['facet', 'The "Rust" Blog', false],
      ['facet', 'ads', true],
      ['text', 'old news', true],
    ]);
    expect(query.clauses[1]).toMatchObject({ quoted: true, start: 5, end: 21 });
    expect(query.clauses[2].field).toBe('source');
  });

  it('keeps URLs and unknown colon-containing text as ordinary search terms', () => {
    expect(parseQuery('https://example.com/foo#bar').clauses[0]).toMatchObject({ kind: 'text', value: 'https://example.com/foo#bar' });
    expect(parseQuery('RFC:123 note:').clauses.map((clause) => [clause.kind, clause.value])).toEqual([
      ['text', 'RFC:123'],
      ['text', 'note:'],
    ]);
    expect(parseQuery('"-not excluded"').clauses[0]).toMatchObject({ exclude: false, value: '-not excluded' });
  });

  it('reports invalid input with positions instead of silently truncating it', () => {
    for (const text of ['"unclosed', '""', '-', 'source:', 'date:2026-02-30', 'sort:random', '-sort:newest', 'date:2026-09-02..2026-09-01', 'date:..']) {
      expect(() => parseQuery(text)).toThrow();
    }
    expect(() => parseQuery('x'.repeat(4097))).toThrow(/4,096/);
    expect(() => parseQuery(Array(17).fill('x').join(' '))).toThrow(/16/);
    expect(() => parseQuery('rust source:')).toThrow(expect.objectContaining({ start: 5, end: 12 }));
    expect(tokenize('"unfinished phrase', true)).toHaveLength(1);
  });

  it('compares UTC calendar days with inclusive ranges, operators and aliases', () => {
    const now = Date.parse('2026-09-08T23:30:00-07:00');
    const matches = (expression: string, day: string) => dateMatches(parseQuery(expression, now).clauses[0], day);
    expect(matches('date:today', '2026-09-09')).toBe(true);
    expect(matches('date:yesterday', '2026-09-08')).toBe(true);
    expect(matches('date:2026-09-01..2026-09-08', '2026-09-08')).toBe(true);
    expect(matches('date:..2026-09-08', '2026-09-09')).toBe(false);
    expect(matches('after:2026-09-08', '2026-09-08')).toBe(false);
    expect(matches('before:2026-09-08', '2026-09-07')).toBe(true);
    expect(matches('date:>=2026-09-08', '2026-09-08')).toBe(true);
    expect(matches('date:<2026-09-08', '2026-09-08')).toBe(false);
    expect(matches('until:2026-09-08', '2026-09-08')).toBe(true);
    expect(matches('date:last7d', '2026-09-02')).toBe(false);
    expect(matches('date:week', '2026-09-03')).toBe(true);
    expect(matches('-date:today', '2026-09-09')).toBe(false);
  });

  it('records the sort and defaults it to relevance', () => {
    expect(parseQuery('rust sort:oldest').sort).toBe('oldest');
    expect(parseQuery('rust').sort).toBe('relevance');
  });
});

describe('canonical addresses', () => {
  it('freezes date shortcuts to absolute UTC dates in shared URLs', () => {
    const now = Date.parse('2026-09-08T23:00:00Z');
    expect(canonicalQuery('rust date:last7d -date:yesterday', now)).toBe('rust date:2026-09-02..2026-09-08 -date:2026-09-07');
    const url = new URL(queryURL('https://example.com/reader/?feed-page=2', 'rust date:year', 2, now));
    expect(url.searchParams.get('q')).toBe('rust date:2025-09-09..2026-09-08');
    expect(url.searchParams.get('search-page')).toBe('2');
    expect(url.searchParams.get('feed-page')).toBe('2');
  });

  it('drops the query and page from an address, and reads them back', () => {
    expect(queryURL('https://example.com/reader/?q=old&search-page=3', '', 1)).toBe('https://example.com/reader/');
    expect(stripSearch('https://example.com/reader/?q=old&search-page=3&x=1')).toBe('https://example.com/reader/?x=1');
    expect(queryFromHref('https://example.com/reader/?q=rust+category:news&search-page=2')).toEqual({ query: 'rust category:news', page: 2 });
    expect(queryFromHref('https://example.com/reader/')).toEqual({ query: null, page: 1 });
  });

  it('quotes values only when a bare token could not carry them', () => {
    expect(quoteValue('hnrss.org')).toBe('hnrss.org');
    expect(quoteValue('Web "Platform"')).toBe('"Web \\"Platform\\""');
    expect(parseQuery(`source:${quoteValue('a b\\c')}`).clauses[0].value).toBe('a b\\c');
  });
});
