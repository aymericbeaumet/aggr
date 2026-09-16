import { describe, expect, it, vi } from "vitest";
import { enhanceMarginNotes, footnoteId, marginNoteId } from "./margin-notes";

describe("margin note decisions", () => {
  it("accepts only same-page fragments and decodes them, keeping a literal fragment that will not decode", () => {
    expect(footnoteId("#fn-1")).toBe("fn-1");
    expect(footnoteId("#fn%201")).toBe("fn 1");
    expect(footnoteId("#fn%E0%A4%A")).toBe("fn%E0%A4%A");
    expect(footnoteId("https://elsewhere.test/#fn-1")).toBeNull();
    expect(footnoteId("")).toBeNull();
  });

  it("names the margin copy after the reference or its position", () => {
    expect(marginNoteId("fnref-1", "fn-1", 0)).toBe("fnref-1-note");
    expect(marginNoteId("", "fn-1", 0)).toBe("fn-1-reference-1-note");
    expect(marginNoteId("", "fn-2", 4)).toBe("fn-2-reference-5-note");
  });

  it("does nothing without a margin column", () => {
    const root = { querySelectorAll: vi.fn() };
    enhanceMarginNotes(root as unknown as ParentNode, { matches: false } as MediaQueryList, {} as Document);
    expect(root.querySelectorAll).not.toHaveBeenCalled();
  });
});
