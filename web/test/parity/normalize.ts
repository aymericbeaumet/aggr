import { JSDOM } from 'jsdom';

/**
 * The canonical form of an HTML fragment, so a Svelte server render and the minijinja render
 * of the same partial compare equal exactly when a browser could not tell them apart.
 *
 * - Comments are dropped (Svelte's hydration markers), then adjacent text nodes are merged.
 * - Attributes are sorted by name and compared verbatim, except that content-hashed asset
 *   names (`assets/<stem>-<12 hex>.<ext>`) lose their hash: the static page links the hashed
 *   file while the client only knows the logical name.
 * - Whitespace runs inside text collapse to one space. A text node loses its leading
 *   whitespace when it starts its parent or follows a block-level sibling, and its trailing
 *   whitespace when it ends its parent or precedes one; a text node left empty disappears.
 *   A space therefore survives only between inline siblings (`<span>1.</span> <a>`), where it
 *   renders. Trimming at the start and end of every element mirrors Svelte's own template
 *   whitespace model; its one blind spot, a space just inside an inline element that follows
 *   inline text (`foo<span> bar</span>`), is a pattern no partial uses.
 * - `<noscript>` content is raw text to the HTML parser (jsdom parses template contents with
 *   scripting on, as browsers do), and the fixture generator serialises that text with its
 *   markup entity-escaped. The raw text is decoded and parsed as markup on both sides, so a
 *   component renders real elements there.
 * - Entity spelling, void-element syntax and attribute order vanish with parsing.
 *
 * The output puts one node per line, indented by depth, so a failing comparison reads as a
 * tree diff.
 */
export function normalize(html: string): string {
  const fragment = parse(html);
  const lines: string[] = [];
  serializeChildren(fragment, 0, lines);
  return lines.join('\n');
}

/**
 * Elements whose surrounding whitespace never renders: block-level boxes, list and table
 * parts, form controls, media frames, metadata and SVG. Everything else is inline
 * (`a`, `span`, `em`, `time`, `code`, `kbd`, `small`, `img`, …), where a space between
 * siblings is visible.
 */
const BLOCK = new Set([
  'address', 'article', 'aside', 'audio', 'base', 'blockquote', 'body', 'button', 'canvas',
  'data', 'datalist', 'dd', 'details', 'dialog', 'div', 'dl', 'dt', 'fieldset', 'figcaption',
  'figure', 'footer', 'form', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'head', 'header', 'hr', 'html',
  'iframe', 'input', 'label', 'legend', 'li', 'link', 'main', 'menu', 'meta', 'nav', 'noscript',
  'object', 'ol', 'optgroup', 'option', 'p', 'path', 'picture', 'pre', 'script', 'section',
  'select', 'source', 'style', 'summary', 'svg', 'table', 'tbody', 'td', 'template', 'textarea',
  'tfoot', 'th', 'thead', 'title', 'tr', 'track', 'ul', 'video',
]);

const VOID = new Set([
  'area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'source', 'track',
  'wbr',
]);

/** `assets/favicon-32-7967c1a90e5a.png` → `assets/favicon-32.png`. */
const HASHED_ASSET = /(assets\/[^\s"'/]+?)-[0-9a-f]{12}(\.[a-z0-9]+)/g;

/** HTML whitespace: the parser's definition, not Unicode's (`&nbsp;` is content). */
const WHITESPACE = /[ \t\n\f\r]+/g;

let dom: JSDOM | undefined;

function document(): Document {
  dom ??= new JSDOM('');
  return dom.window.document;
}

function parse(html: string): DocumentFragment {
  const template = document().createElement('template');
  template.innerHTML = html;
  return template.content;
}

function decodeEntities(text: string): string {
  const textarea = document().createElement('textarea');
  textarea.innerHTML = text;
  return textarea.value;
}

const ELEMENT = 1;
const TEXT = 3;

type Child = { kind: 'element'; element: Element } | { kind: 'text'; text: string };

/** The children that count: comments gone, text merged, whitespace resolved. */
function children(parent: Node): Child[] {
  const nodes: (Element | string)[] = [];
  const source =
    parent.nodeType === ELEMENT && (parent as Element).localName === 'noscript'
      ? parse(decodeEntities(parent.textContent ?? '')).childNodes
      : parent.childNodes;
  for (const node of source) {
    if (node.nodeType === ELEMENT) {
      nodes.push(node as Element);
    } else if (node.nodeType === TEXT) {
      const last = nodes.at(-1);
      const data = (node as Text).data;
      if (typeof last === 'string') {
        nodes[nodes.length - 1] = last + data;
      } else {
        nodes.push(data);
      }
    }
  }
  const out: Child[] = [];
  nodes.forEach((node, index) => {
    if (typeof node !== 'string') {
      out.push({ kind: 'element', element: node });
      return;
    }
    let text = node.replace(WHITESPACE, ' ');
    const previous = nodes[index - 1];
    const next = nodes[index + 1];
    if (previous === undefined || isBlock(previous)) text = text.replace(/^ /, '');
    if (next === undefined || isBlock(next)) text = text.replace(/ $/, '');
    if (text) out.push({ kind: 'text', text });
  });
  return out;
}

function isBlock(node: Element | string): boolean {
  return typeof node !== 'string' && BLOCK.has(node.localName);
}

function serializeChildren(parent: Node, depth: number, lines: string[]): void {
  for (const child of children(parent)) {
    if (child.kind === 'text') {
      lines.push(indent(depth) + escapeText(child.text));
    } else {
      serializeElement(child.element, depth, lines);
    }
  }
}

function serializeElement(element: Element, depth: number, lines: string[]): void {
  const name = element.localName;
  const attributes = [...element.attributes]
    .map((attribute) => [attribute.name, attribute.value.replace(HASHED_ASSET, '$1$2')] as const)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([key, value]) => ` ${key}="${escapeAttribute(value)}"`)
    .join('');
  const open = `${indent(depth)}<${name}${attributes}>`;
  if (VOID.has(name)) {
    lines.push(open);
    return;
  }
  const inner = children(element);
  if (inner.length === 0) {
    lines.push(`${open}</${name}>`);
  } else if (inner.length === 1 && inner[0].kind === 'text') {
    lines.push(`${open}${escapeText(inner[0].text)}</${name}>`);
  } else {
    lines.push(open);
    serializeChildren(element, depth + 1, lines);
    lines.push(`${indent(depth)}</${name}>`);
  }
}

function indent(depth: number): string {
  return '  '.repeat(depth);
}

function escapeText(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;');
}

function escapeAttribute(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/\n/g, '&#10;');
}
