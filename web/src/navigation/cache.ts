/** A page as it came off the network. */
export type Fetched = { url: string; html: string };

export type PageRecord = {
  time: number;
  page: Promise<Fetched>;
  /** Whether the page has arrived. */
  ready: boolean;
  /** The page parsed ahead of time, taken by the swap that uses it. */
  parsed: Document | null;
};

export type CacheOptions = {
  /** Pages kept, least recently used first out. */
  limit?: number;
  /** A page fetched longer ago than this is fetched again rather than shown. */
  lifetime?: number;
  now?: () => number;
  /** Called once a page has arrived, while nothing waits on it. */
  onReady?: (key: string, record: PageRecord, page: Fetched) => void;
};

/**
 * Fetched pages by address. A page is remembered while it is fresh or still on its way. One
 * fetched longer ago than the lifetime may belong to an earlier build, so it is fetched again,
 * and its old copy stands in only when the network cannot answer. A failed request is forgotten.
 */
export class PageCache {
  private readonly pages = new Map<string, PageRecord>();
  readonly limit: number;
  readonly lifetime: number;
  private readonly now: () => number;
  private readonly onReady: CacheOptions['onReady'];

  constructor(
    private readonly request: (key: string) => Promise<Fetched>,
    options: CacheOptions = {},
  ) {
    this.limit = options.limit ?? 24;
    this.lifetime = options.lifetime ?? 5 * 60_000;
    this.now = options.now ?? (() => Date.now());
    this.onReady = options.onReady;
  }

  get size(): number {
    return this.pages.size;
  }

  has(key: string): boolean {
    return this.pages.has(key);
  }

  peek(key: string): PageRecord | undefined {
    return this.pages.get(key);
  }

  keys(): string[] {
    return [...this.pages.keys()];
  }

  clear(): void {
    this.pages.clear();
  }

  /** The page at `key`: from memory while it is fresh or on its way, else from the network. */
  load(key: string): PageRecord {
    const cached = this.pages.get(key);
    if (cached && !(cached.ready && this.now() - cached.time >= this.lifetime)) {
      this.remember(key, cached);
      return cached;
    }
    const record = this.create(key);
    if (cached) {
      record.page = record.page.catch(() => {
        record.time = cached.time;
        record.ready = true;
        return cached.page;
      });
    }
    record.page.catch(() => {
      if (this.pages.get(key) === record) this.pages.delete(key);
    });
    this.remember(key, record);
    return record;
  }

  private create(key: string): PageRecord {
    const record: PageRecord = { time: this.now(), page: Promise.reject(new Error('unset')), ready: false, parsed: null };
    record.page.catch(() => {});
    record.page = this.request(key).then((page) => {
      record.ready = true;
      this.onReady?.(key, record, page);
      return page;
    });
    return record;
  }

  private remember(key: string, record: PageRecord): void {
    this.pages.delete(key);
    this.pages.set(key, record);
    while (this.pages.size > this.limit) {
      const first = this.pages.keys().next();
      if (first.done) break;
      this.pages.delete(first.value);
    }
  }
}
