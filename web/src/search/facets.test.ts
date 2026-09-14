import { describe, expect, it } from 'vitest';
import { readableQuery, resolveFacet } from './facets';

describe('readable initial facet queries', () => {
  const facets = {source:[{value:'spotify-show-123',label:'Underscore_',count:12},{value:'example',label:'Example',count:4}], category:[],tag:[],'published-day':[]};
  it('replaces backing IDs without altering text, exclusions, or equivalent IDs', () => {
    expect(readableQuery('rust -source:spotify-show-123 source:"example"',facets)).toBe('rust -source:"Underscore_" source:"example"');
    expect(readableQuery('source:"Underscore_"',facets)).toBe('source:"Underscore_"');
  });
  it('keeps duplicate-label IDs and tolerates incomplete initial queries', () => {
    expect(readableQuery('source:spotify-show-123',{...facets,source:[...facets.source,{value:'other-show',label:'Underscore_',count:2}]})).toBe('source:spotify-show-123');
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
