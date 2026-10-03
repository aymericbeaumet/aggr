/**
 * Putting the reader where the page they arrived at expects them. Scroll positions are the
 * navigation's to keep: the browser would restore them before the page they belong to had
 * arrived.
 */

/** When the page last moved, to tell a page still gliding from one at rest. */
let scrolled = Number.NEGATIVE_INFINITY;

/**
 * Let go of a page held still after arriving, without putting it back where it arrived: the
 * next page, or the reader asking for a place, decides where it goes from here.
 */
let release: () => void = () => {};

export function settle(): void {
  release();
}

/** Watch the window so `arrive` can tell a fling from a page at rest. */
export function trackScrolling(win: Window = window): void {
  win.addEventListener('scroll', () => (scrolled = performance.now()), { passive: true });
}

/** The element id a fragment names; one that is not valid percent-encoding names itself. */
export function fragmentId(hash: string): string {
  try {
    return decodeURIComponent(hash.slice(1));
  } catch {
    return hash.slice(1);
  }
}

/**
 * Put the reader where the arriving page expects them, and the keyboard with it: the saved
 * place, else the fragment, else the top. A fling or a trackpad's momentum outlives the page it
 * moved and would drag the next one along. Taking the scrollbar away is what makes the
 * compositor let go of it, and until it has, any step it still takes is undone before it is
 * painted. A page at rest is left alone, since a scrollbar's width could shift the layout while
 * it is gone.
 */
export function arrive(saved: number | undefined, hash: string, doc: Document = document): void {
  settle();
  const id = hash ? fragmentId(hash) : '';
  const target = id ? doc.getElementById(id) : null;
  const win = doc.defaultView ?? window;
  const place = () => {
    if (saved !== undefined) win.scrollTo({ top: saved, behavior: 'instant' });
    else if (target) target.scrollIntoView({ block: 'start', behavior: 'instant' });
    else win.scrollTo({ top: 0, behavior: 'instant' });
  };
  const gliding = performance.now() - scrolled < 150;
  const root = doc.documentElement;
  place();
  if (gliding) {
    // Held until the page has been still for a moment (at most 1.5s): a slow frame must not
    // hand the page back to a fling that is still carrying it.
    root.style.overflow = 'hidden';
    const until = performance.now() + 1500;
    let quiet: ReturnType<typeof setTimeout> | undefined;
    const stop = () => {
      clearTimeout(quiet);
      win.removeEventListener('scroll', hold);
      root.style.removeProperty('overflow');
      release = () => {};
    };
    const let_go = () => {
      stop();
      place();
    };
    const hold = () => {
      place();
      clearTimeout(quiet);
      quiet = setTimeout(let_go, performance.now() < until ? 120 : 0);
    };
    win.addEventListener('scroll', hold, { passive: true });
    quiet = setTimeout(let_go, 120);
    release = stop;
  }
  const main = doc.getElementById('content');
  if (main instanceof HTMLElement) {
    main.setAttribute('data-navigation-focus', '');
    main.focus({ preventScroll: true });
  }
}
