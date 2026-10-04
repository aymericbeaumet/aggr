import type { ClientRow } from '../generated/ClientRow';
import type { Completion } from '../search/completion';

/** One page of results, as the list draws them. */
export type ResultPage = { rows: ClientRow[]; total: number; page: number; pages: number; size: number };

export type Scope = { kind: string; slug: string } | null;

/** Where the list is: the page's address, the site root, and the scoped list's own facet. */
export type SearchContext = { root: string; href: string; seed: string; scope: Scope };

/**
 * What the lazily loaded search module does on the state's behalf. The components only ever
 * change the state and call one of these; until the module has loaded, the state alone hides
 * the feed and keeps the text, and the module catches up with it on arrival.
 */
export type SearchDriver = {
  /** The page's list arrived, or the text was set from its address. */
  arrive(runQuery?: boolean): void;
  leave(): void;
  /** The text or cursor changed under the reader's hands. */
  changed(): void;
  /** The cursor moved without an edit. */
  moved(): void;
  focus(): void;
  blur(): void;
  choose(completion: Completion): void;
  highlight(index: number): void;
  /** Escape: close open suggestions. Returns whether any were open. */
  close(): boolean;
  submit(): void;
  clear(): void;
  turn(delta: number): void;
  /** The content or the network changed: revalidate the catalogue and the results. */
  refresh(): void;
};

/**
 * The search field and its results, shared by the toolbar, the suggestion list, the result list
 * and the feed. Persistent across navigations: the engine behind it keeps its catalogue and
 * indexes, while the text follows each page's `?q=`.
 */
class SearchState {
  /** What the field holds. */
  text = $state('');
  cursor = $state(0);
  /** A scoped list's own facet, which the field holds before anything is typed. */
  seed = $state('');
  scope: Scope = null;
  /** Absolute address of the site root. */
  root = '';
  /** The address the list arrived at, without `focus-search`; results are addressed from it. */
  href = '';
  /** Whether a list is on screen for the state to drive. */
  attached = $state(false);
  focused = $state(false);
  /** The field has been used: the clear button may show. */
  revealed = $state(false);
  composing = false;
  /** Results on screen, or null while none are (pending, cleared, or no query). */
  results = $state.raw<ResultPage | null>(null);
  /** The canonical address the results are shown at, which keys their cursor. */
  address = $state('');
  busy = $state(false);
  /** The live region's text: `Searching…`, then the count. */
  status = $state('');
  error = $state('');
  completions = $state.raw<Completion[]>([]);
  /** Whether the suggestion menu is open (it shows only when it also has something to say). */
  open = $state(false);
  highlighted = $state(0);
  page = 1;
  /** Every change bumps it; work started under an older value discards its outcome. */
  generation = 0;
  driver: SearchDriver | null = null;
  private loading: Promise<SearchDriver> | null = null;

  /** Whether the text is a query of its own rather than the scoped list's seed or nothing. */
  get active(): boolean {
    const text = this.text.trim();
    return text !== '' && text !== this.seed.trim();
  }

  get suggesting(): boolean {
    return this.open && this.completions.length > 0;
  }

  get ready(): boolean {
    return this.results !== null;
  }

  /** The clear button shows once the field is in use and holds something. */
  get clearable(): boolean {
    return this.text !== '' && (this.revealed || this.active);
  }

  /** The search module, loaded once on first intent; the chunk stays out of the first paint. */
  load(): Promise<SearchDriver> {
    this.loading ??= import('../search')
      .then((module) => {
        this.driver = module.driver;
        if (this.attached) this.driver.arrive(false);
        return this.driver;
      })
      .catch((error: unknown) => {
        this.loading = null;
        throw error;
      });
    return this.loading;
  }

  /** Run something with the driver: now when it is here, else once it has loaded. */
  drive(run: (driver: SearchDriver) => void): void {
    if (this.driver) run(this.driver);
    else void this.load().then(run).catch(() => {});
  }

  invalidate(): void {
    this.generation += 1;
  }

  /**
   * A list arrived at `href`: the field holds the address's query, else the seed, and the
   * driver takes it from there once loaded.
   */
  arrive(context: SearchContext): void {
    const url = new URL(context.href);
    url.searchParams.delete('focus-search');
    const query = url.searchParams.get('q');
    this.root = context.root;
    this.href = url.href;
    this.seed = context.seed;
    this.scope = context.scope;
    this.text = query ?? context.seed;
    this.cursor = this.text.length;
    this.page = Number(url.searchParams.get('search-page')) || 1;
    this.revealed = false;
    this.error = '';
    this.invalidate();
    this.attached = true;
    if (this.driver) this.driver.arrive();
    else if (query?.trim()) {
      const at = this.href;
      void this.load().then(driver => {
        if (this.attached && this.href === at) driver.arrive();
      }).catch(() => {});
    }
  }

  leave(): void {
    this.attached = false;
    this.invalidate();
    this.driver?.leave();
  }
}

export const search = new SearchState();
