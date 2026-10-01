import type { PaginatorCtx } from '../generated/PaginatorCtx';
import { sitePath } from '../model/urls';
import { positiveInteger } from '../preferences/rules';

/** The query parameter naming a slice of a static page. */
export const FEED_PAGE = 'feed-page';

export type PagerLink = { href: string; visible: boolean };

/**
 * One preferred-size slice of a generated page. Every row stays in the document, so crawlers and
 * readers without JavaScript still see the complete static page; the client hides the rows
 * outside `[start, end)` and points the pager at the neighbouring slices.
 */
export type Slicing = {
  start: number;
  end: number;
  /** 1-based, across every static page and slice. */
  page: number;
  pages: number;
  status: string;
  /** Whether the pager has nothing to offer. */
  hidden: boolean;
  first: PagerLink;
  previous: PagerLink;
  next: PagerLink;
  last: PagerLink;
};

/**
 * The address of slice `slice` of the static page at `target` (a site path, or an absolute
 * address), keeping the current page's other parameters.
 */
export function feedPageUrl(target: string, slice: number, current: string, root: string): string {
  const now = new URL(current);
  const url = new URL(target, root);
  now.searchParams.forEach((value, name) => {
    if (name !== FEED_PAGE && !url.searchParams.has(name)) url.searchParams.set(name, value);
  });
  if (slice > 1) url.searchParams.set(FEED_PAGE, String(slice));
  else url.searchParams.delete(FEED_PAGE);
  return url.href;
}

/** The slice of the page at `current` (its `?feed-page=`) for `preferredSize` rows per page. */
export function sliceFeed(paginator: PaginatorCtx, rowCount: number, preferredSize: number, current: string, root: string): Slicing {
  const staticSize = positiveInteger(paginator.paginate_by, rowCount || 1);
  const staticPage = positiveInteger(paginator.current_index, 1);
  const staticPages = positiveInteger(paginator.number_pagers, 1);
  const totalItems = Math.max(rowCount, positiveInteger(paginator.total_items, rowCount));
  const pageSize = Math.min(positiveInteger(preferredSize, 50), staticSize);
  const slicesPerPage = Math.ceil(staticSize / pageSize);
  const slicesHere = Math.max(1, Math.ceil(rowCount / pageSize));
  const requested = positiveInteger(new URL(current).searchParams.get(FEED_PAGE), 1);
  const slice = Math.min(requested, slicesHere);
  const start = (slice - 1) * pageSize;
  const end = Math.min(start + pageSize, rowCount);

  const lastCount = Math.max(0, totalItems - (staticPages - 1) * staticSize);
  const lastSlices = Math.max(1, Math.ceil(lastCount / pageSize));
  const pages = Math.max(1, (staticPages - 1) * slicesPerPage + lastSlices);
  const page = (staticPage - 1) * slicesPerPage + slice;

  const previous = paginator.previous === null ? current : sitePath(paginator.previous);
  const next = paginator.next === null ? current : sitePath(paginator.next);
  return {
    start,
    end,
    page,
    pages,
    status: `page ${page} / ${pages}`,
    hidden: pages <= 1,
    first: { href: feedPageUrl(sitePath(paginator.first), 1, current, root), visible: page > 1 },
    previous: {
      href: feedPageUrl(slice > 1 ? current : previous, slice > 1 ? slice - 1 : slicesPerPage, current, root),
      visible: page > 1,
    },
    next: {
      href: feedPageUrl(slice < slicesHere ? current : next, slice < slicesHere ? slice + 1 : 1, current, root),
      visible: page < pages,
    },
    last: { href: feedPageUrl(sitePath(paginator.last), lastSlices, current, root), visible: page < pages },
  };
}
