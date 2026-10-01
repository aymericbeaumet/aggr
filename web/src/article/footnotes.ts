import { fragmentId } from '../navigation/arrive';

/**
 * Following a footnote, or its way back, lights up both halves: `:target` already carries the
 * half the fragment names without JavaScript; this marks the other, which is the margin note
 * standing in for the note beside each reference when the notes list is off screen, and the
 * reference itself when the note points back to it.
 */

const ACTIVE = 'data-footnote-active';

/**
 * The margin note the build placed beside a reference, if any. The build writes it right after
 * the reference's `<sup>` and names it `<reference id>-note`; the parser then closes the
 * paragraph around the `<aside>`, so it is found by that name rather than by adjacency.
 */
function marginNote(reference: Element): HTMLElement | null {
  const id = reference.getAttribute('aria-describedby') || (reference.id && `${reference.id}-note`);
  const aside = id ? reference.ownerDocument.getElementById(id) : null;
  return aside instanceof HTMLElement && aside.classList.contains('footnote-margin-note') ? aside : null;
}

/** Mark the halves belonging to the fragment `id` inside `region`, clearing every earlier mark. */
export function pairFootnote(region: HTMLElement, id: string): void {
  for (const marked of region.querySelectorAll(`[${ACTIVE}]`)) marked.removeAttribute(ACTIVE);
  if (!id) return;
  const target = region.ownerDocument.getElementById(id);
  if (!target || !region.contains(target)) return;
  // A note: mark it, and the margin note that stands in for it beside each reference.
  if (target.closest('.footnotes')) {
    target.setAttribute(ACTIVE, '');
    for (const reference of region.querySelectorAll('.footnote-ref a')) {
      if (reference.getAttribute('href') !== `#${id}`) continue;
      marginNote(reference)?.setAttribute(ACTIVE, '');
    }
    return;
  }
  // A reference: mark the marker, so the word it sits against is easy to find again.
  target.closest('.footnote-ref, .citation-ref')?.setAttribute(ACTIVE, '');
}

export function installFootnotes(region: HTMLElement, signal: AbortSignal): void {
  const paired = () => pairFootnote(region, fragmentId(location.hash));
  paired();
  // Where the margin note is on screen, the reference reaches it in place instead of jumping to
  // the notes list at the end; the link stays a link for every other way of following it.
  for (const reference of region.querySelectorAll('.footnote-ref a[data-footnote-ref]')) {
    reference.addEventListener(
      'click',
      (event) => {
        const note = marginNote(reference);
        if (!note || !note.getClientRects().length) return;
        if (event instanceof MouseEvent && (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)) return;
        event.preventDefault();
        note.tabIndex = -1;
        note.focus({ preventScroll: true });
      },
      { signal },
    );
  }
  window.addEventListener('hashchange', paired, { signal });
}
