import { describe, expect, it } from 'vitest';
import { parseQuery, dateMatches, queryFromLocation, queryURL, facetURL, queryHistoryState } from './query';

describe('search query', () => {
  it('retains quoted phrases, escaped quotes, facet IDs and exclusions', () => {
    const query = parseQuery('rust "borrow checker" source:"The \\"Rust\\" Blog" -tag:ads -"old news"');
    expect(query.clauses.map(c => [c.kind, c.value, c.exclude])).toEqual([
      ['text', 'rust', false], ['text', 'borrow checker', false],
      ['facet', 'The "Rust" Blog', false], ['facet', 'ads', true], ['text', 'old news', true]
    ]);
    expect(query.clauses[1]).toMatchObject({ quoted: true });
  });
  it('keeps URLs and unknown colon-containing text as ordinary search terms', () => {
    expect(parseQuery('https://example.com/foo#bar').clauses[0].value).toBe('https://example.com/foo#bar');
    expect(parseQuery('RFC:123 note:').clauses.map(clause => [clause.kind, clause.value])).toEqual([['text', 'RFC:123'], ['text', 'note:']]);
  });
  it('reports invalid input instead of silently truncating it', () => {
    for (const text of ['"unclosed', '""', '-', 'source:', 'date:2026-02-30', 'sort:random', 'x'.repeat(4097), Array(17).fill('x').join(' ')]) {
      expect(() => parseQuery(text)).toThrow();
    }
  });
  it('compares UTC calendar days with inclusive ranges and explicit operators', () => {
    const now = Date.parse('2026-09-08T23:30:00-07:00');
    const matches = (expression: string, day: string) => dateMatches(parseQuery(expression, now).clauses[0], day);
    expect(matches('date:today', '2026-09-09')).toBe(true);
    expect(matches('date:yesterday', '2026-09-08')).toBe(true);
    expect(matches('date:2026-09-01..2026-09-08', '2026-09-08')).toBe(true);
    expect(matches('after:2026-09-08', '2026-09-08')).toBe(false);
    expect(matches('date:>=2026-09-08', '2026-09-08')).toBe(true);
    expect(matches('date:last7d', '2026-09-02')).toBe(false);
  });
  it('reads the unified query without interpreting unrelated URL parameters', () => {
    expect(queryFromLocation(new URL('https://example.com/reader/?q=rust+category:news&source=ignored&sort=oldest'))).toBe('rust category:news');
    expect(queryFromLocation(new URL('https://example.com/reader/?category=news'))).toBe('');
  });
  it('freezes date shortcuts to absolute UTC dates in shared URLs', () => {
    const url = new URL(queryURL('https://example.com/reader/', 'rust date:last7d -date:yesterday', 2, Date.parse('2026-09-08T23:00:00Z')));
    expect(url.searchParams.get('q')).toBe('rust date:2026-09-02..2026-09-08 -date:2026-09-07');
    expect(url.searchParams.get('search-page')).toBe('2');
  });
  it('links facets to the main feed with an exact quoted qualifier', () => {
    const url = new URL(facetURL('https://example.com/reader/', 'category', 'Web "Platform"'));
    expect(url.pathname).toBe('/reader/');
    expect(url.searchParams.get('q')).toBe('category:"Web \\"Platform\\""');
    expect(parseQuery(url.searchParams.get('q')!).clauses[0]).toMatchObject({ kind: 'facet', field: 'category', value: 'Web "Platform"' });
  });
});

it('updates search history while preserving Swup visit state', () => {
  const state = { source: 'swup', url: '/reader/?q=previous', index: 7, random: 0.42 };
  expect(queryHistoryState(state, 'https://example.com/reader/?q=article')).toEqual({
    ...state, url: '/reader/?q=article'
  });
  expect(state.url).toBe('/reader/?q=previous');
  expect(queryHistoryState(null, 'https://example.com/reader/')).toBeNull();
  const foreign = { source: 'other', url: 'unchanged' };
  expect(queryHistoryState(foreign, 'https://example.com/reader/')).toBe(foreign);
});
