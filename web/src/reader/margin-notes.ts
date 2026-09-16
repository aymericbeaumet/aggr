import { $, $$, el } from "./dom";

/** The footnote a reference points at, or null when it is not a same-page fragment. */
export function footnoteId(href: string): string | null {
  if (href.charAt(0) !== "#") return null;
  const id = href.slice(1);
  try { return decodeURIComponent(id); } catch (error) { return id; }
}

/** A stable id for the margin copy of footnote `id`, from the reference's own id or its position. */
export function marginNoteId(referenceId: string, id: string, index: number): string {
  return (referenceId || id + "-reference-" + (index + 1)) + "-note";
}

/** Copy same-page footnotes beside their references when the viewport has room for a margin column. */
export function enhanceMarginNotes(root: ParentNode, viewport: MediaQueryList, document: Document) {
  if (!viewport.matches) return;
  $$(".body", root).forEach(function (body) {
    if (body.dataset.marginNotesEnhanced === "true") return;
    let count = 0;
    $$(".footnote-ref a[data-footnote-ref]", body).forEach(function (reference) {
      const id = footnoteId(reference.getAttribute("href") || "");
      if (id === null) return;
      const definition = document.getElementById(id);
      if (!definition || !body.contains(definition)) return;

      const number = reference.textContent.trim();
      const note = el("aside", {
        "class": "margin-note footnote-margin-note",
        "role": "note",
        "aria-label": "Note " + number,
        "id": marginNoteId(reference.id, id, count)
      });
      Array.prototype.slice.call(definition.childNodes).forEach(function (child) {
        note.appendChild(child.cloneNode(true));
      });
      $$(".footnote-backref", note).forEach(function (backref) { backref.remove(); });
      $$('[id]', note).forEach(function (node) { node.removeAttribute("id"); });
      const marker = el("span", { "class": "margin-note-number", "text": number + ". " });
      const firstParagraph = $("p", note);
      if (firstParagraph) firstParagraph.insertBefore(marker, firstParagraph.firstChild);
      else note.insertBefore(marker, note.firstChild);

      reference.removeAttribute("target");
      reference.removeAttribute("rel");
      reference.setAttribute("aria-describedby", note.id);
      reference.parentElement?.insertAdjacentElement("afterend", note);
      reference.addEventListener("click", function (event) {
        if (!viewport.matches) return;
        event.preventDefault();
        note.setAttribute("tabindex", "-1");
        note.focus({ preventScroll: true });
      });
      count += 1;
    });
    if (count) body.classList.add("has-margin-notes");
    body.dataset.marginNotesEnhanced = "true";
  });
}
