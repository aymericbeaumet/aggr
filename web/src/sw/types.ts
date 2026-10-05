// The configuration the Rust build writes into `sw.js` and the slice of the platform the worker
// touches. The platform types are structural so the real globals and the in-memory fakes of the
// tests both fit without casts.

/** One install-time or offline resource with its exact content revision. */
export interface ResourceEntry {
  url: string;
  revision: string;
  /** An install that cannot fetch a required resource fails, keeping the previous worker. */
  required?: boolean;
}

/** An article family: its page and every retained rendition it needs to read offline. */
export interface OfflineItem {
  url: string;
  title: string;
  resources: ResourceEntry[];
}

/** One file of a published search index, with the size and digest that verify it. */
export interface SearchFile {
  url: string;
  size: number;
  digest: string;
}

/** `search-manifest.json`: the complete index of one build, verifiable file by file. */
export interface SearchManifest {
  version: string;
  base: string;
  files: SearchFile[];
  totalBytes: number;
}

/**
 * `self.AGGR_SW`, defined by `sw.js` before it imports this worker: the cache version, the
 * application and content versions, and the revisioned install-time lists.
 */
export interface SwConfig {
  version: string;
  app_version: string;
  content_version: string;
  precache: ResourceEntry[];
  offline_catalog: OfflineItem[];
  offline_count: number;
  /** The index this build published; its manifest is fetched from `base`. */
  search_manifest: Pick<SearchManifest, 'version' | 'base'>;
}

export type Fetch = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

export interface CacheLike {
  put(request: RequestInfo | URL, response: Response): Promise<void>;
  delete(request: RequestInfo | URL, options?: CacheQueryOptions): Promise<boolean>;
  keys(): Promise<ReadonlyArray<Request>>;
  match(request: RequestInfo | URL, options?: CacheQueryOptions): Promise<Response | undefined>;
}

export interface CacheStorageLike {
  open(name: string): Promise<CacheLike>;
  keys(): Promise<string[]>;
  delete(name: string): Promise<boolean>;
  match(request: RequestInfo | URL, options?: MultiCacheQueryOptions): Promise<Response | undefined>;
}

export interface ClientLike {
  /** Present on window clients; a message from outside the scope is ignored. */
  readonly url?: string;
  postMessage(message: unknown): void;
}

export interface ClientsLike {
  claim(): Promise<void>;
  matchAll(options?: ClientQueryOptions): Promise<ReadonlyArray<ClientLike>>;
}

/** Everything the worker needs from its global scope. */
export interface WorkerEnv {
  /** `registration.scope`: the absolute site root, ending in `/`. */
  scope: string;
  /** `location.origin`; requests to other origins are left alone. */
  origin: string;
  caches: CacheStorageLike;
  fetch: Fetch;
  clients: ClientsLike;
  skipWaiting(): Promise<void>;
  navigationPreload?: { enable(): Promise<void> } | null;
  config: SwConfig;
  /** Test hook; production uses the defaults from `caches.ts`. */
  timeouts?: Partial<Timeouts>;
}

export interface Timeouts {
  /** How long a page request waits for the network before the last good copy answers. */
  network: number;
  /** How long one install-time or offline download may take, body included. */
  precache: number;
}

export interface ExtendableEventLike {
  waitUntil(promise: Promise<unknown>): void;
}

export interface FetchEventLike extends ExtendableEventLike {
  readonly request: Request;
  /** `undefined` wherever navigation preload is unsupported or off. */
  readonly preloadResponse: Promise<Response | undefined>;
  respondWith(response: Promise<Response>): void;
}

export interface MessageEventLike extends ExtendableEventLike {
  readonly data: unknown;
  readonly source: ClientLike | null;
}
