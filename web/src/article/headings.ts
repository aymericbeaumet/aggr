import { navigation } from '../navigation';
import { fragmentId } from '../navigation/arrive';

/**
 * A heading is a place in the article, so the whole of it is the way back to that place, not
 * only the `#` beside it. The heading stays a heading rather than becoming a link: the reader's
 * own selection, and any link the publisher wrote inside it, come first.
 *
 * The sticky stack is measured at the moment of the jump instead of being tracked. The header
 * folds as the page scrolls, so its height is only knowable then, and a jump from the top lands a
 * little low rather than under a header that is about to shrink.
 */

const HEADING = '.body :is(h1, h2, h3, h4, h5, h6)[id]';

/** The heading a click on `target` means, or null when the click belongs to something else. */
export function headingFor(target: EventTarget | null): HTMLElement | null {
  if (!(target instanceof Element)) return null;
  const heading = target.closest(HEADING);
  if (!(heading instanceof HTMLElement)) return null;
  // Anything the publisher made clickable keeps its own behaviour; only the `#` is ours.
  const link = target.closest('a');
  if (link && !link.classList.contains('heading-anchor')) return null;
  return heading;
}

function clearance(): number {
  return [document.querySelector('.top'), document.querySelector('.itemhead')].reduce(
    (total, element) => total + (element?.getBoundingClientRect().height || 0),
    16,
  );
}

export function installHeadings(region: HTMLElement, signal: AbortSignal): void {
  const body = region.querySelector('.body');
  if (!(body instanceof HTMLElement)) return;

  const jump = (id: string, record: boolean) => {
    const target = document.getElementById(id);
    if (!target) return;
    if (record && location.hash.slice(1) !== id) navigation.pushFragment(`#${encodeURIComponent(id)}`);
    const top = window.scrollY + target.getBoundingClientRect().top - clearance();
    window.scrollTo({ top: Math.max(0, top), behavior: 'instant' });
  };

  body.addEventListener(
    'click',
    (event) => {
      if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const heading = headingFor(event.target);
      if (!heading) return;
      if (!getSelection()?.isCollapsed) return;
      event.preventDefault();
      jump(heading.id, true);
    },
    { signal },
  );

  // The browser scrolls a fragment into view against `scroll-padding-top`, which cannot know how
  // tall the folding header is without measuring it on every frame. Correct it once the page has
  // settled, and on every later jump.
  const settle = () => {
    if (signal.aborted) return;
    const id = fragmentId(location.hash);
    if (id && document.getElementById(id)?.closest('.body')) jump(id, false);
  };
  window.addEventListener('hashchange', settle, { signal });
  requestAnimationFrame(settle);
}
