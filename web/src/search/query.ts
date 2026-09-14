import type { Clause, FacetKind, Query, Sort, Token } from './types';

export class QueryError extends Error {
  constructor(message: string, public start = 0, public end = start) { super(message); }
}

export function tokenize(raw: string, incomplete = false): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < raw.length) {
    if (/\s/u.test(raw[i])) { i++; continue; }
    const start = i;
    let value = '', quote = false, quoted = false;
    while (i < raw.length && (quote || !/\s/u.test(raw[i]))) {
      const char = raw[i++];
      if (char === '\\' && i < raw.length && /["\\]/.test(raw[i])) value += raw[i++];
      else if (char === '"') { quote = !quote; quoted = true; }
      else value += char;
    }
    if (quote && !incomplete) throw new QueryError('Close the quoted phrase with a double quote.', start, i);
    tokens.push({ raw: raw.slice(start, i), value, start, end: i, quoted });
  }
  return tokens;
}

export function quoteValue(value: string): string {
  return /[\s"\\]/u.test(value) ? '"' + value.replace(/\\/g, '\\\\').replace(/"/g, '\\"') + '"' : value;
}

const dayMillis = 86400000;
const day = (timestamp: number) => new Date(timestamp).toISOString().slice(0, 10);
function checkedDay(value: string): string {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value) || !Number.isFinite(Date.parse(value)) || day(Date.parse(value)) !== value) {
    throw new QueryError('Use a valid UTC date, such as 2026-09-08.');
  }
  return value;
}
function shifted(value: string, days: number) { return day(Date.parse(value) + days * dayMillis); }

function dates(value: string, operator: string, now: number): Pick<Clause, 'from' | 'through'> {
  const today = day(now);
  if (operator === 'date') {
    if (value === 'today') return { from: today, through: today };
    if (value === 'yesterday') return { from: shifted(today, -1), through: shifted(today, -1) };
    const shortcut = { week: 7, last7d: 7, month: 30, last30d: 30, year: 365 }[value];
    if (shortcut) return { from: shifted(today, 1 - shortcut), through: today };
    const range = value.split('..');
    if (range.length === 2) {
      const from = range[0] ? checkedDay(range[0]) : undefined;
      const through = range[1] ? checkedDay(range[1]) : undefined;
      if ((!from && !through) || (from && through && from > through)) throw new QueryError('The date range must run from an earlier date to a later date.');
      return { from, through };
    }
    const comparison = value.match(/^(>=|<=|>|<|=)(.+)$/);
    if (comparison) return dates(comparison[2], ({ '>=': 'since', '<=': 'until', '>': 'after', '<': 'before', '=': 'date' })[comparison[1]]!, now);
  }
  const exact = checkedDay(value);
  if (operator === 'after') return { from: shifted(exact, 1) };
  if (operator === 'before') return { through: shifted(exact, -1) };
  if (operator === 'since') return { from: exact };
  if (operator === 'until') return { through: exact };
  return { from: exact, through: exact };
}

export function parseQuery(raw: string, now = Date.now()): Query {
  if (raw.length > 4096) throw new QueryError('Search is limited to 4,096 characters.', 4096, raw.length);
  const tokens = tokenize(raw);
  if (tokens.length > 16) throw new QueryError('Use at most 16 search clauses.', tokens[16].start, raw.length);
  let sort: Sort = 'relevance';
  const clauses = tokens.map(token => {
    const exclude = token.value.startsWith('-') && !token.raw.startsWith('"');
    const value = exclude ? token.value.slice(1) : token.value;
    if (!value.trim()) throw new QueryError('Add a word or phrase to search for.', token.start, token.end);
    const clause: Clause = { ...token, value, exclude, kind: 'text' };
    const operator = token.raw.startsWith('"') || token.raw.startsWith('-"') ? null : value.match(/^([a-z-]+):([\s\S]*)$/i);
    if (!operator || /^https?:\/\//i.test(value)) return clause;
    const field = operator[1].toLowerCase(), argument = operator[2];
    if (!['source', 'category', 'tag', 'type', 'date', 'before', 'after', 'since', 'until', 'sort'].includes(field)) return clause;
    if (!argument) throw new QueryError(`Add a value after ${field}:`, token.start, token.end);
    if (['source', 'category', 'tag', 'type'].includes(field)) return { ...clause, kind: 'facet' as const, field: field as FacetKind, value: argument };
    if (['date', 'before', 'after', 'since', 'until'].includes(field)) return { ...clause, kind: 'date' as const, value: argument, ...dates(argument.toLowerCase(), field, now) };
    if (field === 'sort') {
      if (exclude || !['relevance', 'newest', 'oldest'].includes(argument)) throw new QueryError('Sort by relevance, newest, or oldest.', token.start, token.end);
      sort = argument as Sort;
      return { ...clause, kind: 'sort' as const, value: argument };
    }
    return clause;
  });
  return { raw, clauses, sort };
}

export function dateMatches(clause: Clause, date: string): boolean {
  const matches = (!clause.from || date >= clause.from) && (!clause.through || date <= clause.through);
  return clause.exclude ? !matches : matches;
}

export function queryFromLocation(url: URL): string {
  return url.searchParams.get('q') || '';
}

export function canonicalQuery(query: string, now = Date.now()): string {
  let canonical = query;
  for (const clause of parseQuery(query, now).clauses.reverse()) {
    if (clause.kind !== 'date' || !/^(today|yesterday|week|last7d|month|last30d|year)$/.test(clause.value)) continue;
    const value = clause.from === clause.through ? clause.from : `${clause.from || ''}..${clause.through || ''}`;
    canonical = canonical.slice(0, clause.start) + `${clause.exclude ? '-' : ''}date:${value}` + canonical.slice(clause.end);
  }
  return canonical;
}

export function queryURL(base: string, query: string, page = 1, now = Date.now()): string {
  const url = new URL(base);
  if (query.trim()) url.searchParams.set('q', canonicalQuery(query, now));
  if (page > 1) url.searchParams.set('search-page', String(page));
  return url.href;
}

export function facetURL(base: string, field: FacetKind, value: string): string {
  const quoted = '"' + value.replace(/\\/g, '\\\\').replace(/"/g, '\\"') + '"';
  return queryURL(base, `${field}:${quoted}`);
}

export function queryHistoryState(state: unknown, url: string): unknown {
  if (!state || typeof state !== 'object' || !('source' in state) || state.source !== 'swup') return state;
  const destination = new URL(url);
  return { ...state, url: destination.pathname + destination.search + destination.hash };
}
