import { describe, expect, it, vi } from 'vitest';
import { complete, acceptCompletion, completionHighlight, completionMenu, isCompletingFacet, selectedCompletion, stableCompletions } from './completion';
import type { SearchCatalog } from './types';

const facets = { source: [{ value: 'rust-blog', label: 'The Rust Blog', count: 23 }], category: [{ value: 'web-development', label: 'Web Development', count: 9 }], tag: [], 'published-day': [] } satisfies SearchCatalog['facets'];
describe('cursor completion', () => {
  it('inserts readable unique aliases while retaining stable option identities', () => {
    const suggestions = complete('source:rust', 11, facets);
    expect(suggestions[0]).toMatchObject({ id: 'source:rust-blog', label: 'The Rust Blog', insert: 'source:"The Rust Blog"', count: 23 });
  });
  it('replaces only the token at the cursor, preserving exclusions and later clauses', () => {
    const query = '-source:rust tag:code';
    const suggestion = complete(query, 12, facets)[0];
    expect(acceptCompletion(query, suggestion)).toEqual({ query: '-source:"The Rust Blog" tag:code', cursor: 23 });
  });
  it('completes an unfinished quoted facet and leaves punctuation outside untouched', () => {
    const query = 'category:"Web Dev';
    expect(complete(query, query.length, facets)[0].insert).toBe('category:"Web Development"');
  });
  it('offers date shortcuts without suggesting articles or inventing source IDs', () => {
    expect(complete('date:', 5, facets, Date.parse('2026-09-08T12:00:00Z')).map(item => item.insert)).toContain('date:2026-09-08');
    expect(complete('rust', 4, facets)).toEqual([]);
    expect(complete('source:missing', 14, facets)).toEqual([]);
  });
  it('lists every available value for every facet and narrows partial values', () => {
    for (const kind of ['source', 'category', 'tag'] as const) {
      const values = Array.from({ length: 30 }, (_, index) => ({ value: `value-${index}`, label: `Label ${index}`, count: index + 1 }));
      const catalog = { ...facets, [kind]: values };
      expect(complete(`${kind}:`, kind.length + 1, catalog)).toHaveLength(30);
      const query = `${kind}:"Label 2`;
      expect(complete(query, query.length, catalog).map(item => item.insert).sort()).toEqual([2,20,21,22,23,24,25,26,27,28,29].map(index => `${kind}:"Label ${index}"`).sort());
    }
  });
  it('never aliases ambiguous labels or labels that are another stable identifier', () => {
    const catalog = { ...facets, source: [
      { value: 'spotify-show', label: 'Underscore_', count: 4 },
      { value: 'youtube-channel', label: 'Underscore_', count: 2 },
      { value: 'Underscore_', label: 'Other show', count: 1 }
    ] };
    const candidates = complete('source:un', 9, catalog);
    expect(candidates.map(item => item.insert)).toEqual(['source:spotify-show', 'source:youtube-channel', 'source:"Other show"']);
    expect(candidates[0].detail).toContain('spotify-show');
    expect(complete('source:un', 9, { ...catalog, source: [catalog.source[0]] }, Date.now(), catalog)[0].insert).toBe('source:spotify-show');
  });
  it('completes and suppresses type values exactly like other facets', () => {
    const catalog = { ...facets, type: [{ value: 'podcast', label: 'Podcast', count: 12 }] };
    expect(complete('type:po', 7, catalog)[0]).toMatchObject({ id: 'type:podcast', insert: 'type:podcast', count: 12 });
    expect(complete('type:podcast', 12, catalog)).toEqual([]);
    expect(complete('', 0, catalog).map(item => item.insert)).toContain('type:');
  });
  it('suppresses completed values while retaining repairs for open quotes and cursor edits', () => {
    for (const kind of ['source', 'category', 'tag'] as const) {
      const catalog = { ...facets, [kind]: [{value:'ai',label:'Artificial Intelligence',count:12}] };
      for (const query of [`${kind}:ai`, `${kind}:"ai"`, `-${kind}:"Artificial Intelligence"`]) {
        expect(complete(query, query.length, catalog)).toEqual([]);
      }
      const open = `${kind}:"ai`;
      expect(complete(open, open.length, catalog)).toHaveLength(1);
      const editing = `${kind}:"ai"`;
      expect(complete(editing, editing.length - 2, catalog)).toHaveLength(1);
    }
  });
  it('offers all indexed dates for date qualifiers and completes partial boundaries', () => {
    const days = Array.from({length:12},(_,index)=>({value:`2026-09-${String(index+1).padStart(2,'0')}`,label:'',count:1}));
    const catalog = {...facets, 'published-day': days};
    for (const kind of ['date','before','after','since','until']) {
      const query = `${kind}:2026-09-0`;
      expect(complete(query,query.length,catalog)).toHaveLength(9);
      expect(complete(`${kind}:`,kind.length+1,catalog).filter(item=>item.count!==undefined)).toHaveLength(12);
      const finished = `${kind}:2026-09-01`;
      expect(complete(finished,finished.length,catalog)).toEqual([]);
    }
    const query='date:>=2026-09-0';
    expect(complete(query,query.length,catalog)[0].insert).toBe('date:>=2026-09-09');
    const range='date:2026-09-01..2026-09-0';
    expect(complete(range,range.length,catalog)[0].insert).toBe('date:2026-09-01..2026-09-09');
  });
  it('counts contextual date shortcuts and ranges using only dates matching the other filters', () => {
    const catalog = {...facets,'published-day':[{value:'2026-09-08',label:'2026-09-08',count:2},{value:'2026-09-07',label:'2026-09-07',count:3}]};
    const now = Date.parse('2026-09-08T12:00:00Z');
    expect(complete('date:',5,catalog,now,catalog,true).find(item=>item.id==='date:last7d')?.count).toBe(5);
    expect(complete('date:>=',7,catalog,now,catalog,true).find(item=>item.insert==='date:>=2026-09-07')?.count).toBe(5);
    expect(complete('date:',5,{...catalog,'published-day':[]},now,catalog,true)).toEqual([]);
  });
  it('does not reopen completed date or sort values and exposes every qualifier', () => {
    for (const query of ['date:today','date:2026-09-01..2026-09-08','sort:newest','sort:"oldest"']) {
      expect(complete(query,query.length,facets)).toEqual([]);
    }
    expect(complete('sort:',5,facets)).toHaveLength(3);
    expect(complete('',0,facets).map(item=>item.insert)).toEqual(expect.arrayContaining(['source:','category:','tag:','date:','after:','before:','since:','until:','sort:']));
  });
  it('detects incomplete facets without changing quote, exclusion, cursor, or invalid-query semantics', () => {
    for (const kind of ['source', 'category', 'tag', 'type'] as const) {
      const catalog = { ...facets, [kind]: [{value:'example',label:'Example publication',count:12}] };
      for (const query of [`${kind}:`, `${kind}:exa`, `-${kind}:"Example pub`, `${kind}:exa tag:known`]) {
        const cursor = query.includes(' tag:') ? query.indexOf(' tag:') : query.length;
        expect(isCompletingFacet(query, cursor, catalog)).toBe(true);
      }
      for (const query of [`${kind}:example`, `${kind}:"Example publication"`, `${kind}:missing`, `${kind}:exa date:invalid`, `${kind}:exa tag:`]) {
        const cursor = query.indexOf(' ') >= 0 && !query.includes('"') ? query.indexOf(' ') : query.length;
        expect(isCompletingFacet(query, cursor, catalog)).toBe(false);
      }
      const query = `${kind}:example`;
      expect(isCompletingFacet(query, query.length - 2, catalog)).toBe(true);
    }
    expect(isCompletingFacet('date:', 5, facets)).toBe(false);
    expect(isCompletingFacet('ordinary text', 13, facets)).toBe(false);
  });
  it('normalizes a large catalogue once, with linear alias work and constant repeat lookup work', () => {
    const size = 1000;
    const catalog = { ...facets, source: Array.from({length:size}, (_,i) => ({value:`publisher-${i}`,label:`Publisher ${i}`,count:size-i})) };
    const normalize = vi.spyOn(String.prototype, 'normalize');
    try {
      const suggestions = complete('source:', 7, catalog);
      expect(suggestions).toHaveLength(size);
      expect(suggestions[999].insert).toBe('source:"Publisher 999"');
      expect(normalize.mock.calls.length).toBeLessThan(size * 10);
      normalize.mockClear();
      expect(complete('source:publisher  ', 16, catalog)).toHaveLength(size);
      expect(isCompletingFacet('source:pub', 10, catalog)).toBe(true);
      expect(isCompletingFacet('source:publisher-999', 20, catalog)).toBe(false);
      expect(normalize.mock.calls.length).toBeLessThan(20);
    } finally { normalize.mockRestore(); }
  });
  it('uses contextual counts for ranking while resolving aliases against the full catalogue', () => {
    const catalog = { ...facets, source: [{value:'first',label:'Same',count:10},{value:'second',label:'Same',count:8},{value:'third',label:'Unique',count:5}] };
    const scoped = { ...catalog, source: [{...catalog.source[1],count:1},{...catalog.source[2],count:3}] };
    const first = complete('source:', 7, scoped, Date.now(), catalog);
    expect(first.map(item => [item.id,item.insert,item.count])).toEqual([
      ['source:third','source:"Unique"',3], ['source:second','source:second',1]
    ]);
    const refreshed = {...scoped,source:scoped.source.map(facet => ({...facet,count:facet.value==='second'?4:1}))};
    expect(complete('source:',7,refreshed,Date.now(),catalog).map(item=>item.id)).toEqual(['source:second','source:third']);
    expect(complete('source:',7,scoped,Date.now(),catalog)).toEqual(first);
  });
});

describe('completion menu interaction', () => {
  it('accepts the selected stable identity despite reordered or identically labelled options', () => {
    const items = complete('source:',7,{...facets,source:[{value:'one',label:'Same',count:2},{value:'two',label:'Same',count:1}]});
    expect(selectedCompletion(items,'source:two')?.insert).toBe('source:two');
    expect(selectedCompletion([...items].reverse(),'source:two')?.insert).toBe('source:two');
    expect(selectedCompletion([items[0]],'source:two')?.insert).toBe('source:one');
    expect(selectedCompletion([],'source:two')).toBeUndefined();
  });
  it('leaves a moved highlight untouched across store emissions, falls back when it vanishes and leads unmoved lists', () => {
    const items = complete('sort:', 5, facets);
    expect(items.map(item => item.id)).toEqual(['sort:relevance', 'sort:newest', 'sort:oldest']);
    // A store emission during arrow navigation must not write the binding at all, even with a re-created list.
    expect(completionHighlight(items, 'sort:oldest', true)).toBeUndefined();
    expect(completionHighlight(complete('sort:', 5, facets), 'sort:newest', true)).toBeUndefined();
    // The highlighted id vanished from the suggestions: fall back to the first item.
    expect(completionHighlight(items.slice(0, 2), 'sort:oldest', true)).toBe('sort:relevance');
    // Unmoved lists lead with the first item, whatever the binding held before.
    expect(completionHighlight(items, 'sort:oldest', false)).toBe('sort:relevance');
    expect(completionHighlight(items, '', false)).toBe('sort:relevance');
    // An empty list clears the binding so the menu reopens on its first item.
    expect(completionHighlight([], 'sort:oldest', true)).toBe('');
    expect(completionHighlight([], '', false)).toBe('');
  });
  it('keeps accepted and dismissed suggestions closed through focus and asynchronous refreshes', () => {
    let state = completionMenu('idle', 'focus');
    expect(state).toBe('open');
    state = completionMenu(state, 'dismiss');
    expect(completionMenu(state, 'focus')).toBe('dismissed');
    expect(completionMenu(state, 'blur')).toBe('dismissed');
    expect(completionMenu(state, 'input')).toBe('open');
  });
  it('retains identical completion objects and arrays across caret and result updates', () => {
    const first = complete('source:rust', 11, facets);
    const repeated = complete('source:rust', 11, facets);
    expect(stableCompletions(first, repeated)).toBe(first);
    const changed = complete('source:', 7, facets);
    expect(stableCompletions(first, changed)).toBe(changed);
  });
});
