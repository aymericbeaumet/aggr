import { bench, describe } from 'vitest';
import { complete, isCompletingFacet } from './completion';
import type { SearchCatalog } from './types';

for (const size of [100, 1000, 5000]) {
  const catalog: SearchCatalog['facets'] = {
    source: Array.from({length:size}, (_,index) => ({value:`source-${index}`,label:`Publisher ${index}`,count:size-index})),
    category: [], tag: [], 'published-day': []
  };
  complete('source:', 7, catalog);
  describe(`${size} sources`, () => {
    bench('complete all sources', () => { complete('source:', 7, catalog); });
    bench('check partial facet', () => { isCompletingFacet('source:publisher', 16, catalog); });
    bench('check completed facet', () => { isCompletingFacet('source:source-99', 16, catalog); });
  });
}
