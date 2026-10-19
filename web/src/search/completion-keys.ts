import { search } from '../state/search.svelte';

// Keep non-component imports here so suggestion markup edits remain a Vite HMR boundary.
/** The suggestion keys the field handles without submitting or clearing its query. */
export function completionKeydown(event: KeyboardEvent): boolean {
  if (event.key === 'Escape') {
    event.preventDefault();
    event.stopPropagation();
    const wasOpen = search.driver ? search.driver.close() : search.suggesting;
    search.open = false;
    if (!wasOpen && event.currentTarget instanceof HTMLElement) event.currentTarget.blur();
    return true;
  }
  if (!search.suggesting) return false;
  if (event.key === 'Enter' || (event.key === 'Tab' && !event.shiftKey)) {
    event.preventDefault();
    event.stopPropagation();
    const choice = search.completions[search.highlighted] ?? search.completions[0];
    search.driver?.choose(choice);
    return true;
  }
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault();
    event.stopPropagation();
    const length = search.completions.length;
    const next = (search.highlighted + (event.key === 'ArrowDown' ? 1 : -1) + length) % length;
    if (search.driver) search.driver.highlight(next);
    else search.highlighted = next;
    return true;
  }
  return false;
}
