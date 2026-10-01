import { facetIndex, normalized } from './facets';
import {
  canonicalQuery,
  DATE_SHORTCUTS,
  dateMatches,
  dateRange,
  isDateField,
  isFacetKind,
  OPERATORS,
  parseQuery,
  quoteValue,
  quoted,
  SORTS,
  tokenize,
  type DateField,
  type FacetKind,
  type FilterKind,
  type Query,
  type Token,
} from './query';
import type { Facet, Facets } from './types';

/** One suggestion: what it shows, what it inserts, and the span of the query it replaces. */
export type Completion = {
  /** Stable identity (`source:hnrss.org`, `date:today`, `sort:`), whatever the label says. */
  id: string;
  label: string;
  detail: string;
  insert: string;
  count?: number;
  start: number;
  end: number;
};

export type CompletionToken = {
  token: Token | undefined;
  start: number;
  end: number;
  /** `-` when the token is an exclusion, else empty. */
  excluded: string;
  /** The decoded text before the cursor, without its exclusion mark. */
  value: string;
};

/** The token under the cursor, which is the only part completion may replace. */
export function completionToken(query: string, cursor: number): CompletionToken {
  const token = tokenize(query, true).find((entry) => entry.start <= cursor && cursor <= entry.end);
  const start = token?.start ?? cursor;
  const end = token?.end ?? cursor;
  const decoded = tokenize(query.slice(start, cursor), true)[0]?.value || '';
  const excluded = decoded.startsWith('-') ? '-' : '';
  return { token, start, end, excluded, value: excluded ? decoded.slice(1) : decoded };
}

const validToken = (raw: string, now: number): boolean => {
  try {
    parseQuery(raw, now);
    return true;
  } catch {
    return false;
  }
};

const facetInsertion = (kind: FacetKind, facet: Facet, alias: string | undefined, excluded: string): string =>
  excluded + kind + ':' + (alias === undefined || alias === facet.value ? quoteValue(facet.value) : quoted(alias));

const FACET_TOKEN = /^(source|category|tag|type):(.*)$/i;
const DATE_TOKEN = /^(date|before|after|since|until):(.*)$/i;
const EMPTY: Facet[] = [];

/** How many documents the days matching a date clause hold, from the `published-day` values offered. */
function countDays(days: readonly Facet[], clause: { from?: string; through?: string; exclude?: boolean }): number {
  return days.reduce((total, day) => total + (dateMatches(clause, day.value) ? day.count : 0), 0);
}

/**
 * Suggestions for the token under the cursor: qualifier names, facet values with their counts,
 * and date shortcuts. Articles are never suggested; they belong in the result list. `facets`
 * are the values on offer (already scoped to the rest of the query when `scoped`), while
 * `catalogue` names every value the index knows, which is what decides identities and aliases.
 */
export function complete(
  query: string,
  cursor: number,
  facets: Facets | undefined,
  now = Date.now(),
  catalogue: Facets | undefined = facets,
  scoped = false,
): Completion[] {
  const { token, start, end, excluded, value } = completionToken(query, cursor);
  const finished = token !== undefined && cursor === token.end && validToken(token.raw, now);

  const field = value.match(FACET_TOKEN);
  if (field) {
    const kind = field[1].toLowerCase() as FacetKind;
    const fragment = normalized(field[2]);
    const identities = facetIndex(catalogue?.[kind] ?? EMPTY, kind);
    if (finished && identities.lookup(field[2])) return [];
    return facetIndex(facets?.[kind] ?? EMPTY, kind)
      .entries.filter((entry) => entry.search.includes(fragment))
      .map(({ facet }) => {
        const alias = identities.alias(facet);
        return {
          id: `${kind}:${facet.value}`,
          label: facet.label,
          // Name what the clause will actually say, unless the label already says it.
          detail: alias === undefined && facet.label !== facet.value ? `${kind} · ${facet.value}` : kind,
          count: facet.count,
          insert: facetInsertion(kind, facet, alias, excluded),
          start,
          end,
        };
      });
  }

  const date = value.match(DATE_TOKEN);
  if (date) {
    const kind = date[1].toLowerCase() as DateField;
    const argument = date[2].toLowerCase();
    if (finished && !argument.endsWith('..')) return [];
    const days = facets?.['published-day'] ?? EMPTY;
    const relative: Completion[] =
      kind === 'date'
        ? DATE_SHORTCUTS.filter((shortcut) => shortcut.startsWith(argument))
            .map((shortcut) => {
              const count = countDays(days, { ...dateRange(shortcut, 'date', now), exclude: Boolean(excluded) });
              return {
                id: `date:${shortcut}`,
                label: shortcut,
                detail: 'UTC publication date',
                insert: canonicalQuery(`${excluded}date:${shortcut}`, now),
                start,
                end,
                ...(scoped ? { count } : {}),
              };
            })
            .filter((option) => !scoped || option.count)
        : [];
    const range = argument.lastIndexOf('..');
    const boundary = range >= 0 ? argument.slice(0, range + 2) : argument.match(/^(>=|<=|>|<|=)/)?.[0] || '';
    const fragment = argument.slice(boundary.length);
    const exact = days
      .filter((entry) => entry.value.startsWith(fragment))
      .sort((a, b) => b.value.localeCompare(a.value))
      .map((entry): Completion => {
        const insert = `${excluded}${kind}:${boundary}${entry.value}`;
        let count = entry.count;
        if (scoped) {
          try {
            count = countDays(days, { ...dateRange(boundary + entry.value, kind, now), exclude: Boolean(excluded) });
          } catch {
            count = 0;
          }
        }
        return { id: `${kind}:${boundary}${entry.value}`, label: boundary + entry.value, detail: 'UTC publication date', count, insert, start, end };
      })
      .filter((entry) => validToken(entry.insert, now) && (!scoped || (entry.count ?? 0) > 0));
    return [...relative, ...exact];
  }

  const sort = value.match(/^sort:(.*)$/i);
  if (sort) {
    if (finished || excluded) return [];
    return SORTS.filter((option) => option.startsWith(sort[1].toLowerCase())).map((option) => ({
      id: `sort:${option}`,
      label: option,
      detail: 'Sort results',
      insert: `sort:${option}`,
      start,
      end,
    }));
  }

  if (value.includes(':')) return [];
  return OPERATORS.filter((operator) => operator.startsWith(value.toLowerCase())).map((operator) => ({
    id: operator,
    label: operator,
    detail: '',
    insert: excluded + operator,
    start,
    end,
  }));
}

/** Replace only the token under the cursor, leaving the rest of the query alone. */
export function acceptCompletion(query: string, completion: Completion): { query: string; cursor: number } {
  const tail = query.slice(completion.end);
  const space = completion.insert.endsWith(':') || /^\s/.test(tail) ? '' : ' ';
  return {
    query: query.slice(0, completion.start) + completion.insert + space + tail,
    cursor: completion.start + completion.insert.length + space.length,
  };
}

/**
 * The qualifier being edited at the cursor and the query around it: the clauses whose matches
 * scope the values on offer. Undefined when the cursor is not on a facet or date qualifier;
 * throws when the rest of the query is invalid, since its error must still be shown.
 */
export function completionContext(query: string, cursor: number, now = Date.now()): { field: FilterKind; rest: string; query: Query } | undefined {
  const { token, start, end, value } = completionToken(query, cursor);
  if (!token) return undefined;
  const operator = value.match(/^(source|category|tag|type|date|before|after|since|until):/i)?.[1]?.toLowerCase();
  if (!operator) return undefined;
  const field: FilterKind = isDateField(operator) ? 'published-day' : (operator as FacetKind);
  const rest = (query.slice(0, start) + query.slice(end)).trim();
  return { field, rest, query: parseQuery(rest, now) };
}

/**
 * Whether the cursor sits on a facet value still being completed: one that names no known
 * value yet but has candidates, so the reader is choosing rather than searching. Throws when
 * another clause is invalid, since that error takes precedence over any completion.
 */
export function isCompletingFacet(query: string, cursor: number, catalogue: Facets, now = Date.now()): boolean {
  const { token, value } = completionToken(query, cursor);
  const match = value.match(FACET_TOKEN);
  if (!token || !match) return false;
  // Other invalid clauses must still report their own error.
  parseQuery(query.slice(0, token.start) + query.slice(token.end), now);
  const kind = match[1].toLowerCase();
  if (!isFacetKind(kind)) return false;
  const values = catalogue[kind] ?? EMPTY;
  if (validToken(token.raw, now) && facetIndex(values, kind).lookup(match[2])) return false;
  const candidate = complete(query, cursor, catalogue, now)[0];
  if (!candidate) return false;
  try {
    parseQuery(acceptCompletion(query, candidate).query, now);
    return true;
  } catch {
    return false;
  }
}
