import type { ClientRow } from '../generated/ClientRow';

/** One row as the build encodes it into a search document's display metadata, for tests. */
export const row: ClientRow = {
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

/** `aggr_display` as Rust writes it: the JSON's UTF-8 bytes, hex-encoded. */
export const hex = (value: unknown): string =>
  Array.from(new TextEncoder().encode(JSON.stringify(value)), (byte) => byte.toString(16).padStart(2, '0')).join('');
