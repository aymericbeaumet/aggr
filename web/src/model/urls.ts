/**
 * The link rules the templates apply through the `url_for`, `site_path`, `facet_page` and
 * `facet_url` filters (`src/site/render.rs`). `base` is the page's relative path to the site
 * root (`./`, `../`, `../../`, …) and every other path is site-relative (`items/blog/plain/`).
 */

/** A URL scheme (`https:`, `mailto:`, `data:`) means the value is not a site path. */
const SCHEME = /^[a-z][a-z0-9+.-]*:/i;

/** Whether a link target is a site path rather than an absolute URL. */
export function isSiteReference(value: string): boolean {
  return !SCHEME.test(value);
}

/** The path relative to the site root, without a leading `/` or `./` (`site_path`). */
export function sitePath(path: string): string {
  const trimmed = path.replace(/^\/+/, '');
  return trimmed.startsWith('./') ? trimmed.slice(2) : trimmed;
}

/** The href the browser resolves from the current page (`url_for`); `urlFor(base, '')` is `base`. */
export function urlFor(base: string, path: string): string {
  return isSiteReference(path) ? base + sitePath(path) : path;
}

export type FacetKind = 'source' | 'category' | 'tag';

/** The collection page a facet value already has (`facet_page`), e.g. `sources/blog/`. */
export function facetPage(base: string, value: string, kind: FacetKind): string {
  const directory = kind === 'source' ? 'sources' : kind === 'category' ? 'categories' : 'tags';
  return `${base}${directory}/${value}/`;
}

/**
 * The root feed filtered to one facet value (`facet_url`): `?q=kind:"value"`, with the value
 * JSON-quoted and the query form-encoded like Rust's `form_urlencoded` (space is `+`).
 */
export function facetUrl(base: string, value: string, kind: FacetKind): string {
  const q = `${kind}:${JSON.stringify(value)}`;
  return `${base}?${new URLSearchParams({ q }).toString()}`;
}
