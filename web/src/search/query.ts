/**
 * The query language (`docs/client.md`, "Query syntax"): tokens with positions, the clauses
 * they parse into, and the canonical text a shared link carries. Pure; completion, execution
 * and error display all work from the one parse.
 */

export type FacetKind = 'source' | 'category' | 'tag' | 'type';
export type DateField = 'date' | 'before' | 'after' | 'since' | 'until';
/** The Pagefind filters a query can constrain. */
export type FilterKind = FacetKind | 'published-day';
export type Sort = 'relevance' | 'newest' | 'oldest';

export type Token = {
  raw: string;
  /** The text with quotes and escapes resolved. */
  value: string;
  start: number;
  end: number;
  quoted: boolean;
};

export type Clause = Token & {
  kind: 'text' | 'facet' | 'date' | 'sort';
  exclude: boolean;
  field?: FacetKind;
  /** Inclusive UTC day bounds of a date clause; an open side is undefined. */
  from?: string;
  through?: string;
};

export type Query = { raw: string; clauses: Clause[]; sort: Sort };

export const FACET_FIELDS: readonly FacetKind[] = ['source', 'category', 'tag', 'type'];
export const DATE_FIELDS: readonly DateField[] = ['date', 'before', 'after', 'since', 'until'];
export const SORTS: readonly Sort[] = ['relevance', 'newest', 'oldest'];
export const OPERATORS = [
  'source:', 'category:', 'tag:', 'type:', 'date:', 'after:', 'before:', 'since:', 'until:', 'sort:',
];
export const DATE_SHORTCUTS = ['today', 'yesterday', 'last7d', 'last30d', 'year'];
export const MAX_CHARS = 4096;
export const MAX_CLAUSES = 16;

export const isFacetKind = (value: string): value is FacetKind => (FACET_FIELDS as readonly string[]).includes(value);
export const isDateField = (value: string): value is DateField => (DATE_FIELDS as readonly string[]).includes(value);
const isSort = (value: string): value is Sort => (SORTS as readonly string[]).includes(value);

/** A query the reader must correct, with the span of text the message is about. */
export class QueryError extends Error {
  constructor(
    message: string,
    public readonly start = 0,
    public readonly end = start,
  ) {
    super(message);
    this.name = 'QueryError';
  }
}

/**
 * Split a query into tokens, honouring quoted phrases and backslash escapes. An unclosed quote
 * is an error, unless the text is still being typed (`incomplete`).
 */
export function tokenize(raw: string, incomplete = false): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < raw.length) {
    if (/\s/u.test(raw[i])) {
      i++;
      continue;
    }
    const start = i;
    let value = '';
    let quote = false;
    let quoted = false;
    while (i < raw.length && (quote || !/\s/u.test(raw[i]))) {
      const char = raw[i++];
      if (char === '\\' && i < raw.length && /["\\]/.test(raw[i])) value += raw[i++];
      else if (char === '"') {
        quote = !quote;
        quoted = true;
      } else value += char;
    }
    if (quote && !incomplete) throw new QueryError('Close the quoted phrase with a double quote.', start, i);
    tokens.push({ raw: raw.slice(start, i), value, start, end: i, quoted });
  }
  return tokens;
}

/** A value in quotes, with its own quotes and backslashes escaped. */
export function quoted(value: string): string {
  return '"' + value.replace(/\\/g, '\\\\').replace(/"/g, '\\"') + '"';
}

/** A value as a token: bare when it can be, quoted when it holds spaces, quotes or backslashes. */
export function quoteValue(value: string): string {
  return /[\s"\\]/u.test(value) ? quoted(value) : value;
}

const DAY = 86_400_000;
const day = (timestamp: number) => new Date(timestamp).toISOString().slice(0, 10);

function checkedDay(value: string): string {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value) || !Number.isFinite(Date.parse(value)) || day(Date.parse(value)) !== value) {
    throw new QueryError('Use a valid UTC date, such as 2026-09-08.');
  }
  return value;
}

const shifted = (value: string, days: number) => day(Date.parse(value) + days * DAY);

const COMPARISONS: Record<string, DateField> = { '>=': 'since', '<=': 'until', '>': 'after', '<': 'before', '=': 'date' };
const SHORTCUT_DAYS: Record<string, number> = { week: 7, last7d: 7, month: 30, last30d: 30, year: 365 };

/** Resolve a date qualifier to an inclusive range of UTC publication days. */
export function dateRange(value: string, operator: DateField, now: number): { from?: string; through?: string } {
  const today = day(now);
  if (operator === 'date') {
    if (value === 'today') return { from: today, through: today };
    if (value === 'yesterday') return { from: shifted(today, -1), through: shifted(today, -1) };
    const shortcut = SHORTCUT_DAYS[value];
    if (shortcut) return { from: shifted(today, 1 - shortcut), through: today };
    const range = value.split('..');
    if (range.length === 2) {
      const from = range[0] ? checkedDay(range[0]) : undefined;
      const through = range[1] ? checkedDay(range[1]) : undefined;
      if ((!from && !through) || (from && through && from > through)) {
        throw new QueryError('The date range must run from an earlier date to a later date.');
      }
      return { from, through };
    }
    const comparison = value.match(/^(>=|<=|>|<|=)(.+)$/);
    if (comparison) return dateRange(comparison[2], COMPARISONS[comparison[1]], now);
  }
  const exact = checkedDay(value);
  if (operator === 'after') return { from: shifted(exact, 1) };
  if (operator === 'before') return { through: shifted(exact, -1) };
  if (operator === 'since') return { from: exact };
  if (operator === 'until') return { through: exact };
  return { from: exact, through: exact };
}

/** Parse a query, or throw a `QueryError` pointing at what needs correcting. */
export function parseQuery(raw: string, now = Date.now()): Query {
  if (raw.length > MAX_CHARS) throw new QueryError('Search is limited to 4,096 characters.', MAX_CHARS, raw.length);
  const tokens = tokenize(raw);
  if (tokens.length > MAX_CLAUSES) throw new QueryError('Use at most 16 search clauses.', tokens[MAX_CLAUSES].start, raw.length);
  let sort: Sort = 'relevance';
  const clauses = tokens.map((token): Clause => {
    const exclude = token.value.startsWith('-') && !token.raw.startsWith('"');
    const value = exclude ? token.value.slice(1) : token.value;
    if (!value.trim()) throw new QueryError('Add a word or phrase to search for.', token.start, token.end);
    const clause: Clause = { ...token, value, exclude, kind: 'text' };
    // A pasted URL is full text, not a qualifier, however many colons it holds.
    const operator = token.raw.startsWith('"') || token.raw.startsWith('-"') ? null : value.match(/^([a-z-]+):([\s\S]*)$/i);
    if (!operator || /^https?:\/\//i.test(value)) return clause;
    const field = operator[1].toLowerCase();
    const argument = operator[2];
    if (!isFacetKind(field) && !isDateField(field) && field !== 'sort') return clause;
    if (!argument) throw new QueryError(`Add a value after ${field}:`, token.start, token.end);
    if (isFacetKind(field)) return { ...clause, kind: 'facet', field, value: argument };
    if (isDateField(field)) return { ...clause, kind: 'date', value: argument, ...dateRange(argument.toLowerCase(), field, now) };
    if (exclude || !isSort(argument)) throw new QueryError('Sort by relevance, newest, or oldest.', token.start, token.end);
    sort = argument;
    return { ...clause, kind: 'sort', value: argument };
  });
  return { raw, clauses, sort };
}

/** Whether a UTC day satisfies a date clause, exclusion included. */
export function dateMatches(clause: { from?: string; through?: string; exclude?: boolean }, date: string): boolean {
  const matches = (!clause.from || date >= clause.from) && (!clause.through || date <= clause.through);
  return clause.exclude ? !matches : matches;
}

const RELATIVE = /^(today|yesterday|week|last7d|month|last30d|year)$/;

/** Replace relative date shortcuts with absolute dates, so a shared link stays stable. */
export function canonicalQuery(query: string, now = Date.now()): string {
  let canonical = query;
  for (const clause of parseQuery(query, now).clauses.reverse()) {
    if (clause.kind !== 'date' || !RELATIVE.test(clause.value)) continue;
    const value = clause.from === clause.through ? clause.from : `${clause.from || ''}..${clause.through || ''}`;
    canonical = canonical.slice(0, clause.start) + `${clause.exclude ? '-' : ''}date:${value}` + canonical.slice(clause.end);
  }
  return canonical;
}

/** The parameter naming a page of results. */
export const SEARCH_PAGE = 'search-page';

/** The address of a query and page of results, keeping the base's other parameters. */
export function queryURL(base: string, query: string, page = 1, now = Date.now()): string {
  const url = new URL(base);
  if (query.trim()) url.searchParams.set('q', canonicalQuery(query, now));
  else url.searchParams.delete('q');
  if (page > 1) url.searchParams.set(SEARCH_PAGE, String(page));
  else url.searchParams.delete(SEARCH_PAGE);
  return url.href;
}

/** The address without any search of its own. */
export function stripSearch(href: string): string {
  const url = new URL(href);
  url.searchParams.delete('q');
  url.searchParams.delete(SEARCH_PAGE);
  return url.href;
}

/** The query and page an address asks for. */
export function queryFromHref(href: string): { query: string | null; page: number } {
  const url = new URL(href);
  return { query: url.searchParams.get('q'), page: Number(url.searchParams.get(SEARCH_PAGE)) || 1 };
}
