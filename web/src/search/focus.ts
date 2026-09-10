interface Bounds { top: number; bottom: number; left: number; right: number }

export function searchInputVisible(input: Bounds, viewport: Bounds): boolean {
  return input.bottom > input.top && input.right > input.left
    && input.top >= viewport.top && input.bottom <= viewport.bottom
    && input.left >= viewport.left && input.right <= viewport.right;
}

export function revealSearchInput(input: HTMLInputElement | null, doc = document, win = window) {
  if (!input) return;
  const viewport = win.visualViewport;
  const left = viewport?.offsetLeft ?? 0, top = viewport?.offsetTop ?? 0;
  const header = doc.querySelector('.top')?.getBoundingClientRect();
  const tabs = doc.querySelector('.mobile-tabs');
  let bottom = top + (viewport?.height ?? win.innerHeight);
  if (tabs) {
    const style = win.getComputedStyle(tabs);
    if (style.display !== 'none' && style.visibility !== 'hidden') bottom = Math.min(bottom, tabs.getBoundingClientRect().top);
  }
  const bounds = { top: Math.max(top, header?.bottom ?? top), bottom, left, right: left + (viewport?.width ?? win.innerWidth) };
  if (!searchInputVisible(input.getBoundingClientRect(), bounds)) win.scrollTo({ top: 0, left: 0, behavior: 'instant' });
}
