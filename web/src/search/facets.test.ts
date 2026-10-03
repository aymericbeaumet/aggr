import { describe, expect, it } from 'vitest';
import { facetIndex, readableQuery, resolveFacet } from './facets';
import type { Facets } from './types';

const facets: Facets = {
  source: [
    { value: 'open.spotify.com', label: 'Underscore_', count: 12 },
    { value: 'example.com', label: 'Example', count: 4 },
  ],
  category: [
    { value: 'web-development', label: 'Web Development', count: 9 },
    { value: 'news', label: 'News', count: 2 },
  ],
  tag: [],
  'published-day': [],
};

describe('facet resolution', () => {
  it('accepts identifiers and unique labels, case- and width-insensitively', () => {
    expect(resolveFacet('web-development', facets.category ?? [], 'category').value).toBe('web-development');
    expect(resolveFacet('web development', facets.category ?? [], 'category').value).toBe('web-development');
    expect(resolveFacet('ＮＥＷＳ', facets.category ?? [], 'category').value).toBe('news');
    expect(() => resolveFacet('typo', facets.category ?? [], 'category')).toThrow(/Unknown category/);
  });

  it('rejects ambiguous labels and lets an exact identifier win over them', () => {
    const ambiguous = [...(facets.source ?? []), { value: 'youtube.com', label: 'Underscore_', count: 2 }];
    expect(() => resolveFacet('underscore_', ambiguous, 'source')).toThrow(/Ambiguous source/);
    const exact = [...ambiguous, { value: 'underscore_', label: 'Other', count: 1 }];
    expect(resolveFacet('underscore_', exact, 'source').value).toBe('underscore_');
  });

  it('never aliases sources, and aliases other facets only by unique labels', () => {
    expect(facetIndex(facets.source ?? [], 'source').alias((facets.source ?? [])[0])).toBeUndefined();
    const category = facetIndex(facets.category ?? [], 'category');
    expect(category.alias((facets.category ?? [])[0])).toBe('Web Development');
    expect(category.entries.map((entry) => entry.facet.value)).toEqual(['web-development', 'news']);
    expect(facetIndex(facets.category ?? [], 'category')).toBe(category);
  });
});

describe('readable shared queries', () => {
  it('replaces category identifiers with labels but keeps source hostnames and text alone', () => {
    expect(readableQuery('rust -category:web-development source:example.com', facets)).toBe('rust -category:"Web Development" source:example.com');
    expect(readableQuery('category:"Web Development"', facets)).toBe('category:"Web Development"');
  });

  it('keeps duplicate-label identifiers and tolerates incomplete queries', () => {
    const doubled = { ...facets, category: [...(facets.category ?? []), { value: 'webdev', label: 'Web Development', count: 1 }] };
    expect(readableQuery('category:web-development', doubled)).toBe('category:web-development');
    expect(readableQuery('category:', facets)).toBe('category:');
  });
});
