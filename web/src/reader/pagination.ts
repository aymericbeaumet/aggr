/**
 * Client-side feed paging: a statically rendered page of rows is sliced to the preferred page size
 * without another request, and the pager's links point at the right slice of the right static page.
 */

export const FEED_PAGE_PARAMETER = "feed-page";

export function positiveInteger(value: unknown, fallback: number): number {
  const parsed = Number.parseInt(String(value), 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

/** The address of `slice` of the page at `target`, carrying over the current page's other query parameters. */
export function feedPageUrl(target: string, slice: number, current: string, base: string): string {
  const currentUrl = new URL(current);
  const url = new URL(target, base);
  currentUrl.searchParams.forEach(function (value, key) {
    if (key !== FEED_PAGE_PARAMETER && !url.searchParams.has(key)) url.searchParams.set(key, value);
  });
  if (slice > 1) url.searchParams.set(FEED_PAGE_PARAMETER, String(slice));
  else url.searchParams.delete(FEED_PAGE_PARAMETER);
  return url.href;
}

/** What the server put in the pager's data attributes. */
export interface PagerDataset {
  staticPageSize?: string;
  staticPage?: string;
  staticPages?: string;
  totalItems?: string;
  staticFirst?: string;
  staticPrevious?: string;
  staticNext?: string;
  staticLast?: string;
}

export interface PagerLink {
  target: string;
  slice: number;
  visible: boolean;
}

export interface PaginationLayout {
  pageSize: number;
  slice: number;
  /** Rows outside `[start, end)` are hidden. */
  start: number;
  end: number;
  currentPage: number;
  totalPages: number;
  first: PagerLink;
  previous: PagerLink;
  next: PagerLink;
  last: PagerLink;
  /** The slice the address should show: 1 when the preferred size does not split the static page. */
  normalizedSlice: number;
}

/** Lay out `rows` rows of the static page described by `pager` at the preferred size, showing the requested slice. */
export function paginationLayout(rows: number, pager: PagerDataset, preferredSize: unknown, requestedSlice: unknown, current: string): PaginationLayout {
  const staticSize = positiveInteger(pager.staticPageSize, rows || 1);
  const staticPage = positiveInteger(pager.staticPage, 1);
  const staticPages = positiveInteger(pager.staticPages, 1);
  const totalItems = Math.max(rows, positiveInteger(pager.totalItems, rows));
  const pageSize = Math.min(positiveInteger(preferredSize, 50), staticSize);
  const slicesPerFullPage = Math.ceil(staticSize / pageSize);
  const slicesOnPage = Math.max(1, Math.ceil(rows / pageSize));
  const slice = Math.min(positiveInteger(requestedSlice, 1), slicesOnPage);
  const start = (slice - 1) * pageSize;
  const end = Math.min(start + pageSize, rows);

  const finalStaticCount = Math.max(0, totalItems - ((staticPages - 1) * staticSize));
  const finalSlices = Math.max(1, Math.ceil(finalStaticCount / pageSize));
  const totalPages = Math.max(1, ((staticPages - 1) * slicesPerFullPage) + finalSlices);
  const currentPage = ((staticPage - 1) * slicesPerFullPage) + slice;
  const hasPrevious = currentPage > 1;
  const hasNext = currentPage < totalPages;
  const previousTarget = slice > 1 ? current : pager.staticPrevious;
  const previousSlice = slice > 1 ? slice - 1 : slicesPerFullPage;
  const nextTarget = slice < slicesOnPage ? current : pager.staticNext;
  const nextSlice = slice < slicesOnPage ? slice + 1 : 1;

  return {
    pageSize, slice, start, end, currentPage, totalPages,
    first: { target: pager.staticFirst || current, slice: 1, visible: hasPrevious },
    previous: { target: previousTarget || current, slice: previousSlice, visible: hasPrevious },
    next: { target: nextTarget || current, slice: nextSlice, visible: hasNext },
    last: { target: pager.staticLast || current, slice: finalSlices, visible: hasNext },
    normalizedSlice: pageSize < staticSize ? slice : 1
  };
}
