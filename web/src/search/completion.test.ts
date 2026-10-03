import { describe, expect, it } from 'vitest';
import { acceptCompletion, complete, completionContext, isCompletingFacet } from './completion';
import type { Facets } from './types';

const facets: Facets = {
  source: [{ value: 'hnrss.org', label: 'Hacker News', count: 23 }],
  category: [{ value: 'web-development', label: 'Web Development', count: 9 }],
  tag: [],
  'published-day': [],
};
const now = Date.parse('2026-09-08T12:00:00Z');

describe('cursor completion', () => {
  it('suggests qualifiers for a bare token and nothing for free text or unknown values', () => {
    expect(complete('', 0, facets, now).map((item) => item.insert)).toEqual(
      expect.arrayContaining(['source:', 'category:', 'tag:', 'type:', 'date:', 'after:', 'before:', 'since:', 'until:', 'sort:']),
    );
    expect(complete('-sou', 4, facets, now)[0]).toMatchObject({ id: 'source:', insert: '-source:' });
    expect(complete('rust', 4, facets, now)).toEqual([]);
    expect(complete('source:missing', 14, facets, now)).toEqual([]);
  });

  it('inserts hostname identifiers for sources and readable unique labels elsewhere', () => {
    expect(complete('source:hack', 11, facets, now)[0]).toMatchObject({
      id: 'source:hnrss.org', label: 'Hacker News', detail: 'source · hnrss.org', insert: 'source:hnrss.org', count: 23,
    });
    expect(complete('category:"Web Dev', 17, facets, now)[0]).toMatchObject({ insert: 'category:"Web Development"', detail: 'category' });
  });

  it('replaces only the token at the cursor, preserving exclusions and later clauses', () => {
    const query = '-source:hn tag:code';
    const suggestion = complete(query, 10, facets, now)[0];
    expect(acceptCompletion(query, suggestion)).toEqual({ query: '-source:hnrss.org tag:code', cursor: 17 });
    expect(acceptCompletion('sou', { id: 'source:', label: 'source:', detail: '', insert: 'source:', start: 0, end: 3 })).toEqual({ query: 'source:', cursor: 7 });
  });

  it('stops suggesting a value once it is complete, but not while it is being edited', () => {
    for (const query of ['source:hnrss.org', 'source:"hnrss.org"', '-category:"Web Development"']) {
      expect(complete(query, query.length, facets, now)).toEqual([]);
    }
    expect(complete('source:"hnrss', 13, facets, now)).toHaveLength(1);
    expect(complete('source:"hnrss.org"', 16, facets, now)).toHaveLength(1);
  });

  it('never aliases ambiguous labels or labels that are another stable identifier', () => {
    const catalogue: Facets = {
      ...facets,
      category: [
        { value: 'spotify-show', label: 'Underscore_', count: 4 },
        { value: 'youtube-channel', label: 'Underscore_', count: 2 },
        { value: 'Underscore_', label: 'Other show', count: 1 },
      ],
    };
    const offered = complete('category:un', 11, catalogue, now);
    expect(offered.map((item) => item.insert)).toEqual(['category:spotify-show', 'category:youtube-channel', 'category:"Other show"']);
    expect(offered[0].detail).toContain('spotify-show');
    // Identities come from the whole catalogue even when only a scoped subset is offered.
    const scoped = { ...catalogue, category: [catalogue.category![0]] };
    expect(complete('category:un', 11, scoped, now, catalogue)[0].insert).toBe('category:spotify-show');
  });

  it('offers date shortcuts and indexed days, and counts them against the other clauses when scoped', () => {
    const days: Facets = {
      ...facets,
      'published-day': [
        { value: '2026-09-08', label: '2026-09-08', count: 2 },
        { value: '2026-09-07', label: '2026-09-07', count: 3 },
        { value: '2026-08-01', label: '2026-08-01', count: 1 },
      ],
    };
    const inserts = complete('date:', 5, days, now).map((item) => item.insert);
    expect(inserts).toContain('date:2026-09-08');
    expect(inserts).toContain('date:2026-09-02..2026-09-08');
    expect(complete('date:>=2026-09-0', 16, days, now)[0].insert).toBe('date:>=2026-09-08');
    expect(complete('date:2026-09-01..2026-09-0', 26, days, now)[0].insert).toBe('date:2026-09-01..2026-09-08');
    expect(complete('date:today', 10, days, now)).toEqual([]);
    expect(complete('date:', 5, days, now, days, true).find((item) => item.id === 'date:last7d')?.count).toBe(5);
    expect(complete('date:>=', 7, days, now, days, true).find((item) => item.insert === 'date:>=2026-09-07')?.count).toBe(5);
    expect(complete('date:', 5, { ...days, 'published-day': [] }, now, days, true)).toEqual([]);
  });

  it('completes sort values once, never negated', () => {
    expect(complete('sort:', 5, facets, now).map((item) => item.insert)).toEqual(['sort:relevance', 'sort:newest', 'sort:oldest']);
    expect(complete('sort:newest', 11, facets, now)).toEqual([]);
    expect(complete('-sort:', 6, facets, now)).toEqual([]);
  });
});

describe('completion context', () => {
  it('removes only the edited token and names the filter its values come from', () => {
    const query = 'category:web-development "memory safety" source:hn -tag:ads';
    const context = completionContext(query, query.indexOf('source:hn') + 9, now);
    expect(context?.field).toBe('source');
    expect(context?.rest.replace(/\s+/g, ' ')).toBe('category:web-development "memory safety" -tag:ads');
    expect(context?.query.clauses.map((clause) => [clause.kind, clause.value])).toEqual([
      ['facet', 'web-development'], ['text', 'memory safety'], ['facet', 'ads'],
    ]);
    expect(completionContext('rust date:>=2026-', 17, now)?.field).toBe('published-day');
    expect(completionContext('plain text', 5, now)).toBeUndefined();
    expect(() => completionContext('category: source:', 17, now)).toThrow(/Add a value/);
  });

  it('detects a facet still being chosen without changing quote, exclusion or cursor semantics', () => {
    for (const query of ['source:', 'source:hn', '-source:"Hacker N', 'source:hn tag:known']) {
      const cursor = query.includes(' tag:') ? query.indexOf(' tag:') : query.length;
      expect(isCompletingFacet(query, cursor, facets, now)).toBe(true);
    }
    for (const query of ['source:hnrss.org', 'source:"Hacker News"', 'source:missing']) {
      expect(isCompletingFacet(query, query.length, facets, now)).toBe(false);
    }
    // Another clause's error takes precedence over any completion.
    expect(() => isCompletingFacet('source:hn date:invalid', 9, facets, now)).toThrow(/valid UTC date/);
    expect(isCompletingFacet('source:hnrss.org', 14, facets, now)).toBe(true);
    expect(isCompletingFacet('date:', 5, facets, now)).toBe(false);
    expect(() => isCompletingFacet('source:hn sort:bad', 9, facets, now)).toThrow(/Sort by/);
  });
});
