import type { Attachment } from 'svelte/attachments';

/**
 * The children each captured element gave up, by element. A region is captured once, when the
 * page it came from is left, and its nodes may be placed again if the wrapper they were moved
 * into is rebuilt (a key change), so they are remembered rather than read back from the source.
 */
const captured = new WeakMap<Element, ChildNode[]>();

/**
 * Move the children of `content` into the attached element. The nodes are moved, never
 * re-parsed: the article body was sanitized at build time and stays the same DOM. Nodes are
 * appended so that the anchors Svelte keeps inside the wrapper survive, and taken away again
 * when the attachment is torn down. Without content there is nothing to do, which is also what
 * a server render sees.
 */
export function adopt(content: Element | null | undefined): Attachment<Element> | null {
  if (!content) return null;
  return (node) => {
    let nodes = captured.get(content);
    if (!nodes) {
      nodes = [...content.childNodes];
      captured.set(content, nodes);
    }
    node.append(...nodes);
    return () => {
      for (const child of nodes) if (child.parentNode === node) node.removeChild(child);
    };
  };
}
