import type { Preferences } from './aggr';

export interface Token { raw: string; value: string; start: number; end: number; quoted: boolean }
export interface Clause extends Token {
  kind: 'text' | 'facet' | 'date' | 'sort'; exclude: boolean; field?: string;
  from?: string; through?: string;
}
export interface Query { raw: string; clauses: Clause[]; sort: string }
export interface Facet { value: string; label: string; count: number }
export type Facets = Record<string, Facet[]>;
export interface Catalog { version: string; base: string; docs: number; facets: Facets }
export interface Completion { id: string; label: string; detail: string; insert: string; start: number; end: number; count?: number }
export interface Result { url: string; excerpt?: string; meta: Record<string, string> }
export interface ResultRef { id: string; data(): Promise<Result> }
export interface Matches { results: ResultRef[]; filters?: Record<string, Record<string, number>> }
export interface Pagefind {
  init(): Promise<void>; destroy(): Promise<void>;
  preload(term: string | null, options?: Record<string, unknown>): Promise<unknown>;
  search(term: string | null, options?: Record<string, unknown>): Promise<Matches>;
}
export interface PagefindModule { createInstance(options: Record<string, unknown>): Pagefind }
export interface Outcome { results: Result[]; total: number; page: number; pages: number; size: number }
export interface Display {
  original?: string; date?: string; updated?: string; excerpt?: string;
  source_display?: string; source_title?: string; source_query?: string; source_slug?: string;
  feed_sources?: { query_value?: string; slug: string; name: string; display: string }[];
  category?: { slug: string; name: string };
  consumption?: { action: string; minutes?: number; seconds?: number; words?: number };
  discussions?: { name: string; url: string; score?: number }[];
  points?: number;
  comments?: { url: string; count?: number };
  preview?: { url: string; width?: number; height?: number; alt?: string; color?: string; placeholder?: { data_url?: string } };
}
export interface Dates { text(timestamp: number, format: string): string | null; format(): string; render(root?: ParentNode): void }
export interface Selection {
  restore(): void; move(direction: number, focus?: boolean): boolean;
  selected(): HTMLElement | null; link(row: HTMLElement | null): HTMLAnchorElement | null;
}
export interface MountOptions {
  base: string; dates: Dates; preferences?: Preferences; selection?: Selection;
  navigate?(url: string): void; signal?: AbortSignal;
}
export interface SearchHandle { refresh(): void; updateOfflineStatus(): void; destroy(): void }
