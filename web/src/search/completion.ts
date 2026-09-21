import { canonicalQuery, dateMatches, parseQuery, quoteValue } from './query';
import { facetIndex, normalized } from './facets';
import { completionToken } from './completion-token';
import type { Facet, FacetKind, SearchCatalog } from './types';

export interface Completion { id: string; label: string; detail: string; kind?: FacetKind; insert: string; count?: number; start: number; end: number }
const operators = ['source:', 'category:', 'tag:', 'type:', 'date:', 'after:', 'before:', 'since:', 'until:', 'sort:'];
const shortcuts = ['today', 'yesterday', 'last7d', 'last30d', 'year'];
function validToken(raw: string, now: number): boolean {
  try { parseQuery(raw, now); return true; } catch { return false; }
}
const emptyFacets: Facet[] = [];
function facetInsertion(kind: FacetKind, facet: Facet, alias: string | undefined, excluded: string): string {
  if (kind === 'source') return excluded + kind + ':"' + facet.value.replace(/\\/g, '\\\\').replace(/"/g, '\\"') + '"';
  const inserted = alias === undefined || alias === facet.value ? quoteValue(facet.value) : '"' + alias.replace(/\\/g, '\\\\').replace(/"/g, '\\"') + '"';
  return excluded + kind + ':' + inserted;
}

export function complete(query: string, cursor: number, facets: SearchCatalog['facets'] | undefined, now = Date.now(), catalog = facets, contextual = false): Completion[] {
  const { token, start, end, excluded, value } = completionToken(query, cursor);
  const finished = token && cursor === token.end && validToken(token.raw, now);
  const field = value.match(/^(source|category|tag|type):(.*)$/i);
  if (field) {
    const kind = field[1].toLowerCase() as FacetKind, fragment = normalized(field[2]);
    const identities = facetIndex(catalog?.[kind] || emptyFacets);
    if (finished && identities.lookup(field[2])) return [];
    return facetIndex(facets?.[kind] || emptyFacets).entries.filter(entry => entry.search.includes(fragment))
      .map(({facet}) => {
        const alias = identities.alias(facet);
        return { id: kind + ':' + facet.value, kind, label: facet.label, detail: alias === undefined ? `${kind} · ${facet.value}` : kind, count: facet.count, insert: facetInsertion(kind, facet, alias, excluded), start, end };
      });
  }
  const date = value.match(/^(date|before|after|since|until):(.*)$/i);
  if (date) {
    const kind = date[1].toLowerCase(), argument = date[2].toLowerCase();
    if (finished && !argument.endsWith('..')) return [];
    const relative = kind === 'date' ? shortcuts.filter(value => value.startsWith(argument)).map(value => ({ id: 'date:' + value, label: value, detail: 'UTC publication date', insert: canonicalQuery(excluded + 'date:' + value, now), start, end })) : [];
    const range = argument.lastIndexOf('..');
    const boundary = range >= 0 ? argument.slice(0, range + 2) : argument.match(/^(>=|<=|>|<|=)/)?.[0] || '';
    const fragment = argument.slice(boundary.length);
    const days = (facets?.['published-day'] || []).filter(day => day.value.startsWith(fragment))
      .sort((a, b) => b.value.localeCompare(a.value))
      .map(day => ({ id: kind + ':' + boundary + day.value, label: boundary + day.value, detail: 'UTC publication date', count: day.count, insert: excluded + kind + ':' + boundary + day.value, start, end }))
      .filter(day => validToken(day.insert, now));
    if (!contextual) return [...relative, ...days];
    return [...relative, ...days].map(item => {
      const clause = parseQuery(item.insert, now).clauses[0];
      const count = (facets?.['published-day'] || []).reduce((count, day) => count + (dateMatches(clause, day.value) ? day.count : 0), 0);
      return {...item, count};
    }).filter(item => item.count > 0);
  }
  const sort = value.match(/^sort:(.*)$/i);
  if (sort) return finished || excluded ? [] : ['relevance', 'newest', 'oldest'].filter(value => value.startsWith(sort[1].toLowerCase())).map(value => ({ id: 'sort:' + value, label: value, detail: 'Sort results', insert: 'sort:' + value, start, end }));
  if (value.includes(':')) return [];
  const suggestions: Completion[] = operators.filter(operator => operator.startsWith(value.toLowerCase())).map(operator => ({ id: operator, label: operator, detail: '', insert: excluded + operator, start, end }));
  return suggestions;
}

export function isCompletingFacet(query: string, cursor: number, facets: SearchCatalog['facets']): boolean {
  const { token, start, end, excluded, value } = completionToken(query, cursor);
  const field = value.match(/^(source|category|tag|type):(.*)$/i);
  if (!field) return false;
  const kind = field[1].toLowerCase() as FacetKind, index = facetIndex(facets[kind] || emptyFacets);
  if (token && cursor === token.end && validToken(token.raw, Date.now()) && index.lookup(field[2])) return false;
  const fragment = normalized(field[2]);
  const candidate = index.entries.find(entry => entry.search.includes(fragment));
  if (!candidate) return false;
  const insert = facetInsertion(kind, candidate.facet, candidate.alias, excluded);
  try { parseQuery(acceptCompletion(query, {id: '', label: '', detail: '', start, end, insert}).query); return true; } catch { return false; }
}

export function acceptCompletion(query: string, completion: Completion): { query: string; cursor: number } {
  const inserted = completion.insert;
  const tail = query.slice(completion.end);
  const space = inserted.endsWith(':') || /^\s/.test(tail) ? '' : ' ';
  return { query: query.slice(0, completion.start) + inserted + space + tail, cursor: completion.start + inserted.length + space.length };
}

export function selectedCompletion(items: Completion[], selected: string): Completion | undefined {
  return items.find(item => item.id === selected) || items[0];
}

// Decides what the highlight binding should be written with after a store emission (debounced searches,
// status refreshes, delayed counts). `undefined` means "leave the binding alone": a highlight the user moved
// to and that is still listed is owned by the menu, and rewriting it from component state can only race the
// live value. Unmoved lists lead with the first item, a vanished highlight falls back to it, and an empty
// list clears the binding.
export function completionHighlight(items: Completion[], current: string, moved: boolean): string | undefined {
  if (!items.length) return '';
  if (moved && items.some(item => item.id === current)) return undefined;
  return items[0].id;
}

export type CompletionMenu = 'idle' | 'open' | 'dismissed';
export function completionMenu(state: CompletionMenu, event: 'focus' | 'input' | 'dismiss' | 'blur'): CompletionMenu {
  if (event === 'input') return 'open';
  if (event === 'dismiss') return 'dismissed';
  if (state === 'dismissed') return state;
  return event === 'focus' ? 'open' : 'idle';
}

export function stableCompletions(previous: Completion[], next: Completion[]): Completion[] {
  return previous.length === next.length && previous.every((item, index) => {
    const other = next[index];
    return item.id === other.id && item.label === other.label && item.detail === other.detail
      && item.insert === other.insert && item.count === other.count
      && item.start === other.start && item.end === other.end;
  }) ? previous : next;
}
