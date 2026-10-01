/** The shapes search works with: the build's catalogue and Pagefind's runtime, as it sees them. */

/** One value of a facet, with its readable label and how many documents carry it. */
export type Facet = { value: string; label: string; count: number };

/** Facet values by filter name (`source`, `category`, `tag`, `type`, `published-day`). */
export type Facets = Record<string, Facet[] | undefined>;

/**
 * `search-catalog.json`: the completion vocabulary and where the index of that version lives,
 * with nothing of the index itself.
 */
export type Catalog = { version: string; base: string; docs: number; facets: Facets };

/** A hydrated Pagefind result: `meta.aggr_display` carries the row the reader draws. */
export type PagefindResult = { url: string; excerpt?: string; meta: Record<string, string> };

/** A match before hydration; `data()` fetches the fragment. */
export type ResultRef = { id: string; data(): Promise<PagefindResult> };

export type Matches = {
  results: ResultRef[];
  /** Counts per filter value among these results, for the filters whose index is loaded. */
  filters?: Record<string, Record<string, number>>;
};

export type Pagefind = {
  init(): Promise<void>;
  destroy(): Promise<void>;
  /** Load every filter index, so search results come with complete counts. */
  filters?(): Promise<unknown>;
  preload(term: string | null, options?: Record<string, unknown>): Promise<unknown>;
  search(term: string | null, options?: Record<string, unknown>): Promise<Matches>;
};

export type PagefindModule = { createInstance(options: Record<string, unknown>): Pagefind };
