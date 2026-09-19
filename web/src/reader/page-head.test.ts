import { describe, expect, it, vi } from "vitest";
import { PAGE_HEAD_SELECTOR, syncPageHead } from "./page-head";

describe("page head synchronisation", () => {
  it("names every per-page tag and none of the shell's", () => {
    for (const selector of ['link[rel="canonical"]', 'meta[property^="og:"]', 'link[rel="alternate"]', 'script[type="application/ld+json"]', 'meta[name^="aggr:"]']) {
      expect(PAGE_HEAD_SELECTOR.split(",")).toContain(selector);
    }
    expect(PAGE_HEAD_SELECTOR).not.toMatch(/stylesheet|manifest|viewport|icon|charset/);
  });

  it("removes the current page's metadata and imports the incoming document's", () => {
    const stale = { remove: vi.fn() };
    const fresh = { name: "fresh" };
    const imported = { name: "imported" };
    const head = {
      querySelectorAll: vi.fn(() => [stale]),
      appendChild: vi.fn(),
      ownerDocument: { importNode: vi.fn(() => imported) }
    };
    const incoming = { head: { querySelectorAll: vi.fn(() => [fresh]) } };
    syncPageHead(head as unknown as HTMLHeadElement, incoming as unknown as Document);
    expect(head.querySelectorAll).toHaveBeenCalledWith(PAGE_HEAD_SELECTOR);
    expect(stale.remove).toHaveBeenCalledTimes(1);
    expect(head.ownerDocument.importNode).toHaveBeenCalledWith(fresh, true);
    expect(head.appendChild).toHaveBeenCalledWith(imported);
  });

  it("leaves the head alone without an incoming document", () => {
    const head = { querySelectorAll: vi.fn(() => []), appendChild: vi.fn(), ownerDocument: { importNode: vi.fn() } };
    syncPageHead(head as unknown as HTMLHeadElement, null);
    syncPageHead(head as unknown as HTMLHeadElement, {} as Document);
    expect(head.querySelectorAll).not.toHaveBeenCalled();
  });
});
