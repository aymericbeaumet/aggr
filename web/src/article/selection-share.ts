import { mount, unmount } from 'svelte';
import SelectionShare from './SelectionShare.svelte';

/**
 * Sharing a passage. A shared selection addresses words, not DOM offsets, so the link survives a
 * rebuild: the article's text is indexed once per page, a selection becomes a word range, and a
 * link opened with `#selection=` puts the same words back under selection. The address bar is
 * never rewritten while selecting; only the link the Share action copies carries the range. The
 * toolbar answers the reader's own gesture, never a restored selection.
 */

/** A word range as it travels in a fragment: base64url of `start,end`. */
export function encodeSelection(start: number, end: number): string {
  return btoa(`${start},${end}`).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

export function decodeSelection(token: string): [number, number] | null {
  try {
    const plain = atob(token.replace(/-/g, '+').replace(/_/g, '/'));
    if (!/^\d+,\d+$/.test(plain)) return null;
    const [start, end] = plain.split(',').map(Number);
    return Number.isSafeInteger(start) && Number.isSafeInteger(end) && end > start ? [start, end] : null;
  } catch {
    return null;
  }
}

/** The range a page fragment carries, or null when it carries none. */
export function sharedSelection(hash: string): [number, number] | null {
  const shared = /(?:^|[#&])selection=([A-Za-z0-9_-]+)/.exec(hash)?.[1];
  return shared ? decodeSelection(shared) : null;
}

export type Word = { start: number; end: number };

/** The words of a text, as character spans. */
export function words(text: string): Word[] {
  return Array.from(text.matchAll(/\S+/g), (match) => ({ start: match.index, end: match.index + match[0].length }));
}

/** The article's text nodes laid end to end, and the words across them. */
export type TextIndex = { text: string; nodes: Array<{ node: Text; start: number }>; words: Word[] };

export function indexText(root: Node): TextIndex {
  const walker = (root.ownerDocument ?? document).createTreeWalker(root, NodeFilter.SHOW_TEXT);
  let text = '';
  const nodes: TextIndex['nodes'] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (!(node instanceof Text)) continue;
    nodes.push({ node, start: text.length });
    text += node.data;
  }
  return { text, nodes, words: words(text) };
}

/** The words touched by the characters `from..to`, as a half-open word range; null for none. */
export function wordRange(index: TextIndex, from: number, to: number): [number, number] | null {
  if (to <= from) return null;
  const start = index.words.findIndex((word) => word.end > from);
  let end = index.words.length;
  while (end > 0 && index.words[end - 1].start >= to) end -= 1;
  return start < 0 || end <= start ? null : [start, end];
}

export type Located = { start: [Text, number]; end: [Text, number] };

/** Where a word range sits in the text nodes; null when the range is past the article. */
export function locate(index: TextIndex, [first, last]: [number, number]): Located | null {
  const head = index.words[first];
  const tail = index.words[last - 1];
  if (!head || !tail) return null;
  const from = lastNode(index, (start) => start <= head.start);
  const to = lastNode(index, (start) => start < tail.end);
  if (!from || !to) return null;
  return { start: [from.node, head.start - from.start], end: [to.node, tail.end - to.start] };
}

/** The last text node whose start satisfies `where`: the one a character offset falls in. */
function lastNode(index: TextIndex, where: (start: number) => boolean): TextIndex['nodes'][number] | undefined {
  for (let at = index.nodes.length - 1; at >= 0; at -= 1) {
    if (where(index.nodes[at].start)) return index.nodes[at];
  }
  return undefined;
}

/** How far into the article's text `(node, offset)` is; null when it is not in the article. */
function position(body: HTMLElement, node: Node, offset: number): number | null {
  if (!body.contains(node)) return null;
  const probe = body.ownerDocument.createRange();
  try {
    probe.setStart(body, 0);
    probe.setEnd(node, offset);
    return probe.toString().length;
  } catch {
    return null;
  }
}

export function installSelectionShare(body: HTMLElement, signal: AbortSignal): void {
  let token = '';
  const toolbar = mount(SelectionShare, {
    target: document.body,
    props: {
      link: () => {
        const url = new URL(location.href);
        url.hash = `selection=${token}`;
        return url.href;
      },
    },
  });
  let indexed: TextIndex | undefined;
  const index = () => (indexed ??= indexText(body));
  let gestured = false;
  let frame = 0;
  let reposition = false;
  let restoreFrame = 0;

  function place(): void {
    frame = 0;
    if (signal.aborted) return;
    const selection = getSelection();
    if (!selection?.rangeCount || selection.isCollapsed) {
      toolbar.hide();
      return;
    }
    const range = selection.getRangeAt(0);
    if (!reposition) {
      const from = position(body, range.startContainer, range.startOffset);
      const to = position(body, range.endContainer, range.endOffset);
      const span = from === null || to === null ? null : wordRange(index(), from, to);
      if (!span) {
        toolbar.hide();
        return;
      }
      token = encodeSelection(span[0], span[1]);
    }
    reposition = false;
    if (!gestured) return;
    const box = range.getBoundingClientRect();
    if (!box.width && !box.height) return;
    toolbar.show(box);
  }
  const schedule = () => {
    reposition = false;
    if (!frame) frame = requestAnimationFrame(place);
  };
  // Scrolling with a selection moves the toolbar without deriving the range again.
  const follow = () => {
    if (!toolbar.shown() || frame) return;
    reposition = true;
    frame = requestAnimationFrame(place);
  };
  document.addEventListener('selectionchange', schedule, { signal });
  document.addEventListener('pointerdown', () => (gestured = true), { signal });
  document.addEventListener(
    'keydown',
    (event) => {
      if (event.shiftKey || ((event.metaKey || event.ctrlKey) && event.key === 'a')) gestured = true;
    },
    { signal },
  );
  window.addEventListener('scroll', follow, { passive: true, signal });
  window.addEventListener('resize', follow, { signal });

  function restore(): void {
    const range = sharedSelection(location.hash);
    if (!range || signal.aborted) return;
    const found = locate(index(), range);
    if (!found) return;
    const selected = document.createRange();
    selected.setStart(...found.start);
    selected.setEnd(...found.end);
    const selection = getSelection();
    selection?.removeAllRanges();
    selection?.addRange(selected);
    found.start[0].parentElement?.scrollIntoView({ block: 'center', behavior: 'instant' });
  }
  restoreFrame = requestAnimationFrame(() => {
    if (!signal.aborted) restoreFrame = requestAnimationFrame(restore);
  });
  window.addEventListener('hashchange', restore, { signal });

  signal.addEventListener(
    'abort',
    () => {
      cancelAnimationFrame(frame);
      cancelAnimationFrame(restoreFrame);
      unmount(toolbar);
    },
    { once: true },
  );
}
