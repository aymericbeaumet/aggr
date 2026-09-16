// Share what you highlighted. A selection inside an article body is addressed by the word range it
// covers, so the link survives a rebuild that changes markup but not wording, and it needs nothing
// from the server: the reader restores the same selection on arrival.

const PARAM = "selection";

interface TextIndex {
  text: string;
  nodes: { node: Text; start: number }[];
}

/** Every word in the body, as character offsets into the body's concatenated text. */
function indexBody(body: HTMLElement): TextIndex & { words: { start: number; end: number }[] } {
  const walker = body.ownerDocument.createTreeWalker(body, NodeFilter.SHOW_TEXT);
  const nodes: { node: Text; start: number }[] = [];
  let text = "";
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const value = (node as Text).data;
    if (!value) continue;
    nodes.push({ node: node as Text, start: text.length });
    text += value;
  }
  return { text, nodes, words: wordSpans(text) };
}

/** Character spans of every word, in document order. */
export function wordSpans(text: string): { start: number; end: number }[] {
  const words: { start: number; end: number }[] = [];
  const pattern = /\S+/g;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    words.push({ start: match.index, end: match.index + match[0].length });
  }
  return words;
}

/** The half-open word range a character span touches, or `null` when it touches no word. */
export function wordRange(
  words: { start: number; end: number }[],
  from: number,
  to: number
): [number, number] | null {
  const start = words.findIndex(word => word.end > from);
  let end = -1;
  for (let at = words.length - 1; at >= 0; at--) {
    if (words[at].start < to) { end = at + 1; break; }
  }
  return start < 0 || end <= start ? null : [start, end];
}

/** Character offset of a range boundary, counted the same way `indexBody` counts. */
function charOffset(body: HTMLElement, container: Node, offset: number): number | null {
  if (!body.contains(container.nodeType === Node.TEXT_NODE ? container.parentNode : container)) return null;
  const probe = body.ownerDocument.createRange();
  try {
    probe.setStart(body, 0);
    probe.setEnd(container, offset);
  } catch {
    return null;
  }
  return probe.toString().length;
}

function boundary(index: TextIndex, offset: number): { node: Text; offset: number } | null {
  for (let at = index.nodes.length - 1; at >= 0; at--) {
    const entry = index.nodes[at];
    if (offset >= entry.start && offset <= entry.start + entry.node.data.length) {
      return { node: entry.node, offset: offset - entry.start };
    }
  }
  return null;
}

export function encodeSelection(start: number, end: number): string {
  return btoa(`${start},${end}`).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function decodeSelection(token: string): [number, number] | null {
  try {
    const plain = atob(token.replace(/-/g, "+").replace(/_/g, "/"));
    const [start, end] = plain.split(",").map(Number);
    return Number.isInteger(start) && Number.isInteger(end) && start >= 0 && end > start ? [start, end] : null;
  } catch {
    return null;
  }
}

/** `#selection=<token>` in a URL fragment, alongside any ordinary anchor. */
export function selectionToken(hash: string): string | null {
  const match = /(?:^|[#&])selection=([A-Za-z0-9_-]+)/.exec(hash);
  return match ? match[1] : null;
}

export function mountSelectionSharing(root: ParentNode, signal: AbortSignal) {
  const body = root.querySelector<HTMLElement>("article.item .body");
  if (!body || signal.aborted) return;
  const doc = body.ownerDocument;
  const view = doc.defaultView;
  if (!view) return;

  const toolbar = doc.createElement("div");
  toolbar.className = "selection-share";
  toolbar.hidden = true;
  const share = doc.createElement("button");
  share.type = "button";
  share.className = "selection-share-action";
  share.textContent = "Share";
  toolbar.append(share);
  doc.body.append(toolbar);
  signal.addEventListener("abort", () => toolbar.remove(), { once: true });

  let token: string | null = null;
  let frame: number | null = null;
  let repositionOnly = false;
  let resetLabel: ReturnType<typeof setTimeout> | undefined;
  // Arriving through a shared link restores its selection; the toolbar belongs to the reader's own
  // gesture, so it waits for one that can change a selection.
  let gestured = false;
  // Walking the article costs real time on a long page, and the article does not change under a
  // page scope, so the word index is built once.
  let cached: ReturnType<typeof indexBody> | undefined;
  const index = () => (cached ??= indexBody(body!));

  function shareURL(): string {
    const url = new URL(view!.location.href);
    url.hash = token ? `${PARAM}=${token}` : "";
    return url.href;
  }

  function hide() {
    toolbar.hidden = true;
    if (!token) return;
    token = null;
    // Leaving a stale fragment behind would share the wrong words.
    view!.history.replaceState(view!.history.state, "", shareURL());
  }

  function place(range: Range) {
    if (!gestured) return;
    const rect = range.getBoundingClientRect();
    if (!rect.width && !rect.height) return hide();
    toolbar.hidden = false;
    const width = toolbar.offsetWidth;
    const left = Math.min(
      Math.max(rect.left + rect.width / 2 - width / 2, 8),
      Math.max(8, view!.innerWidth - width - 8)
    );
    toolbar.style.left = `${left + view!.scrollX}px`;
    toolbar.style.top = `${rect.top + view!.scrollY - toolbar.offsetHeight - 10}px`;
  }

  function refresh() {
    const reposition = repositionOnly;
    frame = null;
    repositionOnly = false;
    const selection = doc.getSelection();
    if (!selection || selection.isCollapsed || selection.rangeCount === 0) return hide();
    const range = selection.getRangeAt(0);
    // Scrolling and resizing move the same selection: follow it, do not re-derive it.
    if (reposition) return void place(range);
    const from = charOffset(body!, range.startContainer, range.startOffset);
    const to = charOffset(body!, range.endContainer, range.endOffset);
    if (from === null || to === null || to <= from) return hide();
    const words = wordRange(index().words, from, to);
    if (!words) return hide();
    const next = encodeSelection(words[0], words[1]);
    if (next !== token) {
      token = next;
      view!.history.replaceState(view!.history.state, "", shareURL());
      share.textContent = "Share";
    }
    place(range);
  }

  function schedule(reposition = false) {
    repositionOnly = frame === null ? reposition : repositionOnly && reposition;
    if (frame !== null) return;
    frame = view!.requestAnimationFrame(refresh);
  }

  function follow() {
    if (!toolbar.hidden) schedule(true);
  }

  doc.addEventListener("selectionchange", () => schedule(), { signal });
  doc.addEventListener("pointerdown", () => { gestured = true; }, { signal });
  doc.addEventListener(
    "keydown",
    event => {
      const selectAll = (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "a";
      if (event.shiftKey || selectAll) gestured = true;
    },
    { signal }
  );
  view.addEventListener("resize", follow, { signal });
  view.addEventListener("scroll", follow, { passive: true, signal });

  share.addEventListener("click", async () => {
    const url = shareURL();
    clearTimeout(resetLabel);
    try {
      if (navigator.share) await navigator.share({ title: doc.title, url });
      else {
        await navigator.clipboard.writeText(url);
        share.textContent = "Copied";
        resetLabel = setTimeout(() => { share.textContent = "Share"; }, 1600);
      }
    } catch {
      share.textContent = "Copy failed";
      resetLabel = setTimeout(() => { share.textContent = "Share"; }, 1600);
    }
  }, { signal });

  /** Reselect the shared words so the recipient sees exactly what the sender highlighted. */
  function restore(shared: string) {
    const range = decodeSelection(shared);
    if (!range) return;
    const words = index();
    const first = words.words[range[0]];
    const last = words.words[range[1] - 1];
    if (!first || !last) return;
    const from = boundary(words, first.start);
    const to = boundary(words, last.end);
    if (!from || !to) return;
    const selected = doc.createRange();
    selected.setStart(from.node, from.offset);
    selected.setEnd(to.node, to.offset);
    const selection = doc.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(selected);
    // The fragment already describes this selection, so keep it instead of clearing it.
    token = shared;
    (from.node.parentElement ?? body!).scrollIntoView({ block: "center", behavior: "auto" });
  }

  const shared = selectionToken(view.location.hash);
  if (shared) restore(shared);
}
