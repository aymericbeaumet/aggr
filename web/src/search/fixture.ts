import type { ClientRow } from '@aggr-model/ClientRow';

/** One row as the build encodes it into a search document's display metadata, for tests. */
const plain: ClientRow = {
  path: 'items/blog/2026/09/plain',
  url: 'items/blog/2026/09/plain/',
  link: 'https://blog.example/plain',
  title: 'A plain article',
  language: null,
  excerpt: 'The summary the build wrote.',
  age_band: 'h24',
  category: 'news',
  labels: [{ name: 'Rust', slug: 'rust' }],
  preview: null,
  metadata: {
    original: 'https://blog.example/plain',
    date: '2026-09-08T10:00:00+00:00',
    source_slug: 'blog.example',
    source_query: 'blog.example',
    source_display: 'blog.example',
    source_title: 'Blog',
    feed_sources: [],
    is_aggregated: false,
    word_count: 120,
    reading_minutes: 1,
    discussions: [],
  },
};

/** Search carries the same typed row used by feed pages. */
export function searchRow(value: ClientRow): ClientRow {
  return value;
}

export const row = plain;
export const hex = (value: unknown): string =>
  Array.from(new TextEncoder().encode(JSON.stringify(value)), (byte) => byte.toString(16).padStart(2, '0')).join('');
