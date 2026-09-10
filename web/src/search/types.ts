export type FacetKind = 'source' | 'category' | 'tag' | 'type';
export type FilterKind = FacetKind | 'published-day';
export type Sort = 'relevance' | 'newest' | 'oldest';
export interface Token { raw: string; value: string; start: number; end: number; quoted: boolean }
export interface Clause extends Token {
  kind: 'text' | 'facet' | 'date' | 'sort';
  exclude: boolean;
  field?: FacetKind;
  from?: string;
  through?: string;
}
export interface Query { raw: string; clauses: Clause[]; sort: Sort }
export interface Facet { readonly value: string; readonly label: string; readonly count: number }
export interface SearchCatalog {
  version: string; base: string; docs: number;
  facets: Record<Exclude<FilterKind, 'type'>, readonly Facet[]> & { type?: readonly Facet[] };
}
export interface ResultData {
  readonly url: string; readonly excerpt?: string; readonly meta: Readonly<Record<string, string>>; readonly filters?: Readonly<Record<string, string[]>>;
}
export interface ResultRef { id: string; data: () => Promise<ResultData> }
export interface PagefindMatches { results: ResultRef[]; filters?: Record<string, Record<string, number>> }
export interface PagefindAPI {
  init(): Promise<void>; options(options: Record<string, unknown>): Promise<void>;
  destroy(): Promise<void>;
  filters(): Promise<Record<string, Record<string, number>>>;
  preload(query: string | null, options?: Record<string, unknown>): Promise<unknown>;
  search(query: string | null, options?: Record<string, unknown>): Promise<PagefindMatches>;
}
export interface PagefindModule { createInstance(options: Record<string, unknown>): PagefindAPI }
export interface SearchPage { results: ResultData[]; total: number; page: number; pages: number; size: number }
export interface SearchOptions {
  root: HTMLElement; staticFeed: HTMLElement | null; resultsRoot: HTMLElement; base: string;
  session: SearchSession;
  preferences: { values: Record<string, unknown> };
  navigate: (url: string) => void; onRowsChanged: () => void;
  getOfflineStatus: () => unknown; onQueryChanged: () => void; replaceLocation: (url: string) => void;
  /** Move the result selection without leaving the search field; false when there are no rows. */
  moveSelection: (direction: number) => boolean;
  /** Open the selected result; false when nothing is selected. */
  openSelected: () => boolean;
}
export interface SearchHandle { destroy(): Promise<void>; focus(options?: { restore?: boolean }): void; refresh(): void; isActive(): boolean; updateOfflineStatus(): void; updateDates(): void; select(url: string): void }

/** Rust display::Metadata, transported inside opaque Pagefind display metadata. */
export interface ItemMetadata {
  original?: string; date?: string; updated?: string;
  source_slug?: string; source_display?: string; source_title?: string;
  is_aggregated?: boolean; feed_display?: string;
  category?: { slug: string; name: string };
  word_count?: number; reading_minutes?: number;
  consumption?: { action: 'read' | 'listen' | 'watch'; minutes?: number; seconds?: number; words?: number };
  discussions?: { name: string; url: string; score?: number }[];
  points?: number;
  comments?: { url: string; count?: number };
}
import type { SearchSession } from './engine';
