import { describe, expect, it } from "vitest";
import { feedPageUrl, paginationLayout, positiveInteger } from "./pagination";

const base = "https://reader.test/nested/";

describe("feed page addresses", () => {
  it("keeps the current query except the page, and only writes a page above one", () => {
    const current = base + "?q=rust&feed-page=3";
    expect(feedPageUrl("page/2/", 2, current, base)).toBe(base + "page/2/?q=rust&feed-page=2");
    expect(feedPageUrl("page/2/", 1, current, base)).toBe(base + "page/2/?q=rust");
    expect(feedPageUrl(current, 1, current, base)).toBe(base + "?q=rust");
  });

  it("lets the target's own parameters win", () => {
    expect(feedPageUrl(base + "?q=svelte", 4, base + "?q=rust&sort=old", base)).toBe(base + "?q=svelte&sort=old&feed-page=4");
  });

  it("reads positive integers and falls back otherwise", () => {
    expect(positiveInteger("25", 50)).toBe(25);
    expect(positiveInteger("0", 50)).toBe(50);
    expect(positiveInteger("-3", 50)).toBe(50);
    expect(positiveInteger(undefined, 50)).toBe(50);
    expect(positiveInteger("12abc", 50)).toBe(12);
  });
});

describe("pagination layout", () => {
  const current = base + "page/2/";
  const pager = { staticPageSize: "50", staticPage: "2", staticPages: "3", totalItems: "120", staticFirst: base, staticPrevious: base, staticNext: base + "page/3/", staticLast: base + "page/3/" };

  it("shows everything on one slice when the preferred size covers the static page", () => {
    const layout = paginationLayout(50, pager, "50", null, current);
    expect(layout).toMatchObject({ pageSize: 50, slice: 1, start: 0, end: 50, currentPage: 2, totalPages: 3, normalizedSlice: 1 });
    expect(layout.first).toEqual({ target: base, slice: 1, visible: true });
    expect(layout.previous).toEqual({ target: base, slice: 1, visible: true });
    expect(layout.next).toEqual({ target: base + "page/3/", slice: 1, visible: true });
    expect(layout.last).toEqual({ target: base + "page/3/", slice: 1, visible: true });
  });

  it("splits a static page into slices and walks them before moving to the next static page", () => {
    const first = paginationLayout(50, pager, "25", null, current);
    expect(first).toMatchObject({ pageSize: 25, slice: 1, start: 0, end: 25, currentPage: 3, totalPages: 5, normalizedSlice: 1 });
    expect(first.previous).toEqual({ target: base, slice: 2, visible: true });
    expect(first.next).toEqual({ target: current, slice: 2, visible: true });
    const second = paginationLayout(50, pager, "25", "2", current);
    expect(second).toMatchObject({ slice: 2, start: 25, end: 50, currentPage: 4, normalizedSlice: 2 });
    expect(second.previous).toEqual({ target: current, slice: 1, visible: true });
    expect(second.next).toEqual({ target: base + "page/3/", slice: 1, visible: true });
    expect(second.last).toEqual({ target: base + "page/3/", slice: 1, visible: true });
  });

  it("clamps a slice past the end and hides the pager for a single page", () => {
    const clamped = paginationLayout(50, pager, "10", "99", current);
    expect(clamped).toMatchObject({ slice: 5, start: 40, end: 50, currentPage: 10, totalPages: 12 });
    const single = paginationLayout(3, { staticPageSize: "3", staticPage: "1", staticPages: "1", totalItems: "3" }, "50", null, base);
    expect(single).toMatchObject({ totalPages: 1, currentPage: 1 });
    expect(single.first.visible && single.previous.visible && single.next.visible && single.last.visible).toBe(false);
    expect(single.first.target).toBe(base);
  });

  it("counts a short final static page precisely", () => {
    const last = paginationLayout(20, { ...pager, staticPage: "3" }, "10", "2", base + "page/3/");
    expect(last).toMatchObject({ pageSize: 10, slice: 2, currentPage: 12, totalPages: 12 });
    expect(last.next.visible).toBe(false);
    expect(last.last).toEqual({ target: base + "page/3/", slice: 2, visible: false });
  });

  it("falls back to the row count without pager metadata and never divides by zero", () => {
    expect(paginationLayout(0, {}, "50", null, base)).toMatchObject({ pageSize: 1, slice: 1, start: 0, end: 0, totalPages: 1 });
    expect(paginationLayout(7, {}, "abc", "abc", base)).toMatchObject({ pageSize: 7, slice: 1, end: 7, totalPages: 1 });
  });
});
