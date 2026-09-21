import { describe, expect, it } from 'vitest';
import { readableQuery, resolveFacet } from './facets';
import aliasCases from '../../../tests/fixtures/source-filter-aliases.json';

describe('readable initial facet queries', () => {
  const facets = {source:[{value:'hnrss.org',label:'Hacker News: Front Page',count:12},{value:'example',label:'Example',count:4}], category:[],tag:[],'published-day':[]};
  it('keeps source hosts canonical without altering text, exclusions, or display names', () => {
    expect(readableQuery('rust -source:hnrss.org source:"example"',facets)).toBe('rust -source:hnrss.org source:"example"');
    expect(readableQuery('source:"Hacker News: Front Page"',facets)).toBe('source:"hnrss.org"');
  });
  it('keeps duplicate-label IDs and tolerates incomplete initial queries', () => {
    expect(readableQuery('source:hnrss.org',{...facets,source:[...facets.source,{value:'other-show',label:'Hacker News: Front Page',count:2}]})).toBe('source:hnrss.org');
    expect(readableQuery('source:',facets)).toBe('source:');
  });
  it('normalizes Unicode aliases but preserves exact identifier precedence across catalogue replacements', () => {
    const initial = [{value:'fullwidth',label:'ＮＥＷＳ',count:2}];
    expect(resolveFacet('news',initial,'source').value).toBe('fullwidth');
    const ambiguous = [...initial,{value:'plain',label:'News',count:3}];
    expect(() => resolveFacet('news',ambiguous,'source')).toThrow(/Ambiguous/);
    const replacement = [...ambiguous,{value:'news',label:'Exact identifier',count:1}];
    expect(resolveFacet('news',replacement,'source').value).toBe('news');
    expect(resolveFacet('news',initial,'source').value).toBe('fullwidth');
  });
});

describe('source filters retain canonical identifiers despite presentation aliases', () => {
  for (const { name, facets } of aliasCases) {
    it(name, () => {
      for (const facet of facets) {
        const query = readableQuery('source:' + JSON.stringify(facet.value), {source: facets, category: [], tag: [], 'published-day': []});
        expect(query).toBe('source:' + JSON.stringify(facet.expected));
        const value = facet.expected;
        expect(resolveFacet(value, facets, 'source').value).toBe(facet.value);
      }
    });
  }
});
