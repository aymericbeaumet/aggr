import { queryHistoryState } from './search/query';

interface NavigationPorts {
  location: Pick<Location, 'href' | 'assign'>;
  history: Pick<History, 'state' | 'replaceState'>;
  base(): string;
  swup(): { location: URL; navigate(target: string): unknown } | undefined;
  releaseReady(): boolean;
  beforeNavigate(): void;
  reselectCurrent(): void;
}

export function createNavigation(ports: NavigationPorts) {
  let currentURL = ports.location.href;
  return {
    currentURL: () => currentURL,
    acceptPage() { currentURL = ports.location.href; },
    replaceLocation(target: string) {
      const url = new URL(target, ports.base()).href;
      if (ports.location.href !== url) ports.history.replaceState(queryHistoryState(ports.history.state, url), '', url);
      currentURL = url;
      const swup = ports.swup();
      if (swup) swup.location.href = url;
    },
    navigate(target: string) {
      const destination = new URL(target, ports.base()).href;
      if (destination === ports.location.href) {
        ports.reselectCurrent();
        return;
      }
      ports.beforeNavigate();
      const swup = ports.swup();
      if (ports.releaseReady() || !swup) ports.location.assign(destination);
      else swup.navigate(destination);
    }
  };
}

/** Promote intent without duplicating work or letting speculative routes grow unbounded. */
export function enqueuePrefetch(queue: readonly string[], target: string, urgent: boolean, limit = 8): string[] {
  if (queue.includes(target)) return urgent ? [target, ...queue.filter(url => url !== target)] : [...queue];
  return urgent ? [target, ...queue].slice(0, limit) : [...queue.slice(0, Math.max(0, limit - 1)), target];
}

/**
 * The cache key for a speculative page load, or null when the link leaves the site, is not a page, or is the
 * current page. Fragments and the transient `focus-search` flag never make a page distinct.
 */
export function prefetchKey(target: string, resolveBase: string, scope: string, current: { pathname: string; search: string }): string | null {
  let url: URL;
  try { url = new URL(target, resolveBase); } catch { return null; }
  const site = new URL(scope);
  if (url.origin !== site.origin || url.pathname.indexOf(site.pathname) !== 0 || url.pathname.slice(-1) !== '/') return null;
  url.hash = '';
  url.searchParams.delete('focus-search');
  const key = url.pathname + url.search;
  return key === current.pathname + current.search ? null : key;
}

export function keyboardObscuresNavigation(editing: boolean, layoutHeight: number, visibleHeight: number, scale: number): boolean {
  return editing && scale === 1 && layoutHeight - visibleHeight > 150;
}

export function mountMobileNavigation(bar: HTMLElement, window: Window) {
  const document = bar.ownerDocument;
  const viewport = window.visualViewport;
  const resize = new ResizeObserver(() => measure());
  let disposed = false;
  function measure() {
    document.documentElement.style.setProperty('--bottom-nav-offset', `${bar.getBoundingClientRect().height}px`);
  }
  function keyboard() {
    if (disposed) return;
    const active = document.activeElement;
    const editing = active instanceof HTMLElement && (active.isContentEditable || active.matches('input:not([type=checkbox]):not([type=radio]):not([type=button]), textarea, select'));
    bar.toggleAttribute('data-keyboard-open', editing && !!viewport && keyboardObscuresNavigation(true, window.innerHeight, viewport.height, viewport.scale));
  }
  function reselect(event: MouseEvent) {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const link = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>('a[href]') : null;
    if (!link || link.href !== window.location.href) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    const active = document.activeElement;
    if (active instanceof HTMLElement && (active.isContentEditable || active.matches('input, textarea, select'))) active.blur();
    window.scrollTo({ top: 0, behavior: 'instant' });
  }
  resize.observe(bar, { box: 'border-box' });
  measure();
  keyboard();
  bar.addEventListener('click', reselect, true);
  viewport?.addEventListener('resize', keyboard);
  document.addEventListener('focusin', keyboard);
  const afterFocus = () => queueMicrotask(keyboard);
  document.addEventListener('focusout', afterFocus);
  return () => {
    disposed = true;
    resize.disconnect();
    bar.removeEventListener('click', reselect, true);
    viewport?.removeEventListener('resize', keyboard);
    document.removeEventListener('focusin', keyboard);
    document.removeEventListener('focusout', afterFocus);
    document.documentElement.style.removeProperty('--bottom-nav-offset');
    bar.removeAttribute('data-keyboard-open');
  };
}
