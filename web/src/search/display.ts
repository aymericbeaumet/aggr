import type { ClientRow } from '../generated/ClientRow';
import type { PagefindResult } from './types';

/**
 * The row a result draws. Display metadata travels hex-encoded in a zero-weight Pagefind
 * field, so provider names and JSON keys can never become search terms; decoded, it is the
 * same `ClientRow` the feed renders.
 */

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value);

/** Whether a decoded value has what `Row` and `Metadata` dereference. */
export function isClientRow(value: unknown): value is ClientRow {
  if (!isObject(value)) return false;
  if (!['path', 'url', 'link', 'title', 'excerpt', 'age_band'].every((key) => typeof value[key] === 'string')) return false;
  if (value.language !== null && typeof value.language !== 'string') return false;
  if (value.category !== null && typeof value.category !== 'string') return false;
  if (!Array.isArray(value.labels) || !value.labels.every((label) => isObject(label) && typeof label.slug === 'string' && typeof label.name === 'string')) return false;
  const preview = value.preview;
  if (preview !== null) {
    if (!isObject(preview) || typeof preview.url !== 'string' || typeof preview.width !== 'number' || typeof preview.height !== 'number') return false;
    if (!isObject(preview.placeholder) || typeof preview.placeholder.data_url !== 'string') return false;
  }
  const metadata = value.metadata;
  if (!isObject(metadata)) return false;
  if (!['original', 'date', 'source_slug', 'source_query', 'source_display', 'source_title'].every((key) => typeof metadata[key] === 'string')) return false;
  return Array.isArray(metadata.feed_sources) && Array.isArray(metadata.discussions);
}

function decode(result: PagefindResult): ClientRow | null {
  try {
    const hex = result.meta?.aggr_display || '';
    if (!/^(?:[0-9a-f]{2})+$/i.test(hex)) return null;
    const bytes = Uint8Array.from(hex.match(/../g) || [], (part) => Number.parseInt(part, 16));
    const value: unknown = JSON.parse(new TextDecoder().decode(bytes));
    return isClientRow(value) ? value : null;
  } catch {
    return null;
  }
}

const rows = new WeakMap<PagefindResult, ClientRow | null>();

/** The row of a hydrated result, decoded once per result snapshot. */
export function displayRow(result: PagefindResult): ClientRow | null {
  let row = rows.get(result);
  if (row === undefined) rows.set(result, (row = decode(result)));
  return row;
}

const ENTITIES: Record<string, string> = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", '#39': "'" };

/** Pagefind's excerpt as text: its `<mark>`s and any other markup gone, entities decoded. */
export function excerptText(html: string): string {
  return html
    .replace(/<[^>]*>/g, '')
    .replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (entity, name: string) => {
      const lower = name.toLowerCase();
      if (lower.startsWith('#x')) return String.fromCodePoint(Number.parseInt(lower.slice(2), 16));
      if (lower.startsWith('#')) return String.fromCodePoint(Number(lower.slice(1)));
      return ENTITIES[lower] ?? entity;
    })
    .replace(/\s+/g, ' ')
    .trim();
}

/**
 * The row to draw for a result of one query: the decoded row, with Pagefind's excerpt in
 * place of the article's own when the query matched something in it. An excerpt that
 * highlights nothing (a query of filters alone, or one that matched the metadata) says less
 * than the article's summary, which the feed row shows too.
 */
export function resultRow(result: PagefindResult): ClientRow | null {
  const row = displayRow(result);
  if (!row) return null;
  const excerpt = result.excerpt;
  if (!excerpt || !excerpt.includes('<mark')) return row;
  const text = excerptText(excerpt);
  return text ? { ...row, excerpt: text } : row;
}
