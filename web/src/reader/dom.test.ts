import { afterEach, describe, expect, it, vi } from "vitest";
import { $, $$, el, setStyle } from "./dom";

describe("dom helpers", () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  it("queries the given root, or the document when the root is missing", () => {
    const found = { id: "hit" };
    const root = { querySelector: vi.fn(() => found), querySelectorAll: vi.fn(() => [found, found]) };
    const document = { querySelector: vi.fn(() => null), querySelectorAll: vi.fn(() => []) };
    vi.stubGlobal("document", document);
    expect($("#hit", root as unknown as ParentNode)).toBe(found);
    expect($$(".row", root as unknown as ParentNode)).toEqual([found, found]);
    expect($("#hit", null)).toBeNull();
    expect($$(".row")).toEqual([]);
    expect(document.querySelector).toHaveBeenCalledWith("#hit");
  });

  it("builds elements with text, markup, attributes and only the truthy children", () => {
    const node = { textContent: "", innerHTML: "", setAttribute: vi.fn(), appendChild: vi.fn() };
    vi.stubGlobal("document", { createElement: vi.fn(() => node) });
    const child = { nodeName: "SPAN" } as unknown as Node;
    el("aside", { "class": "note", role: "note", text: "Hello", hidden: false, empty: null }, [child, null, false]);
    expect(node.textContent).toBe("Hello");
    expect(node.setAttribute.mock.calls).toEqual([["class", "note"], ["role", "note"], ["hidden", "false"], ["empty", "null"]]);
    expect(node.appendChild).toHaveBeenCalledTimes(1);
    el("p", { html: "<b>x</b>", text: null });
    expect(node.innerHTML).toBe("<b>x</b>");
    expect(node.textContent).toBe("");
  });

  it("writes a style property only when its value changes", () => {
    const style = { value: "", getPropertyValue: vi.fn(() => style.value), setProperty: vi.fn((_: string, next: string) => { style.value = next; }) };
    const node = { style } as unknown as HTMLElement;
    setStyle(node, "--x", 0.5);
    setStyle(node, "--x", "0.5");
    expect(style.setProperty).toHaveBeenCalledTimes(1);
    setStyle(node, "--x", 1);
    expect(style.setProperty).toHaveBeenLastCalledWith("--x", "1");
  });
});
