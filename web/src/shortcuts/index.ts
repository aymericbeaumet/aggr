import { flushSync } from 'svelte';
import { navigation } from '../navigation';
import { sitePath } from '../model/urls';
import { page } from '../state/page.svelte';
import { preferences } from '../state/preferences.svelte';
import { selection } from '../state/selection.svelte';
import type { CursorRow } from '../selection/cursor';
import { openShortcutHelp } from './help';
import { editing, externalTarget, gotoTarget, scrollDistance, type Subject } from './keys';
import { focusSearch } from './search';

/** The article on screen, as the external shortcuts see it. */
function subject(): Subject | null {
  const view = page.model?.page;
  if (view?.view === 'article') {
    const header = view.data.header;
    return {
      original: header.metadata.original || null,
      link: header.link,
      title: header.title,
      discussions: header.metadata.discussions.map((discussion) => ({ name: discussion.name, href: discussion.url })),
    };
  }
  return selection.current;
}

/** One line of prose, for the scroll keys to step by. */
function lineHeight(): number {
  const content = document.querySelector('.body') || document.querySelector('main');
  if (!content) return 24;
  const style = getComputedStyle(content);
  return parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.6 || 24;
}

function pageScroll(key: string, byLine: boolean): void {
  const line = lineHeight();
  const header = (document.querySelector('.itemhead') || document.querySelector('.top'))?.getBoundingClientRect().height || 0;
  const distance = scrollDistance(line, window.innerHeight - header, preferences.scrollAmount, byLine);
  window.scrollBy({ top: key === 'd' || key === 'e' ? distance : -distance, behavior: 'instant' });
}

/** The link of the row under the cursor, once the cursor has been drawn. */
function selectedLink(row: CursorRow | null): HTMLAnchorElement | null {
  if (!row) return null;
  flushSync();
  const link = document.querySelector(`.rows .row[data-path="${CSS.escape(row.path)}"] [data-row-open]`);
  return link instanceof HTMLAnchorElement ? link : null;
}

/** Put the keyboard and the viewport on the row now selected, and fetch what Enter would open. */
function reveal(row: CursorRow | null, focus = true): boolean {
  const link = selectedLink(row);
  if (!link) return false;
  if (focus) link.focus({ preventScroll: true });
  link.closest('.row')?.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'instant' });
  // The row under the cursor is the one Enter opens next.
  navigation.prefetch(link.href);
  return true;
}

const isArticle = () => page.kind === 'item';

let gotoArmed = false;
let gotoTimer: ReturnType<typeof setTimeout> | undefined;

export function installShortcuts(): void {
  document.addEventListener('keydown', (event) => {
    if (event.defaultPrevented) return;
    const single = preferences.singleKeyShortcuts;
    const inEditor = editing(event.target);
    const dialog = document.querySelector('dialog[open]');
    const here = location.href;

    // Cmd/Ctrl+K reaches the search field from anywhere, including from inside an editor.
    if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === 'k') {
      event.preventDefault();
      focusSearch();
      return;
    }

    // Scroll keys work with Ctrl even when single-key shortcuts are off.
    const key = event.key.length === 1 ? event.key.toLowerCase() : event.key;
    if (!event.altKey && !event.metaKey && !inEditor && !(event.ctrlKey && event.shiftKey)) {
      const byLine = event.ctrlKey && (key === 'e' || key === 'y');
      // Held with Ctrl, d and u are the shortcut the help offers to anyone who turned the
      // single-key ones off; on their own they need those to be on.
      const byHalfPage = (event.key === 'd' || event.key === 'u') && (event.ctrlKey || single);
      if ((byLine || byHalfPage) && !dialog) {
        event.preventDefault();
        pageScroll(key, byLine);
        return;
      }
    }

    if (inEditor || event.metaKey || event.ctrlKey || event.altKey || !single) return;

    if (gotoArmed) {
      gotoArmed = false;
      clearTimeout(gotoTimer);
      const target = gotoTarget(event.key, page.root, page.model?.site.entries ?? []);
      if (target) {
        event.preventDefault();
        if ('top' in target) {
          if (isArticle()) window.scrollTo({ top: 0, behavior: 'instant' });
          else reveal(selection.edge('first', here));
        } else void navigation.go(target.url);
      }
      return;
    }

    if (dialog) return;

    if (event.key === '?') {
      event.preventDefault();
      openShortcutHelp();
      return;
    }
    // Search from anywhere without a modifier. A page with no field of its own lands on the feed
    // with it focused, the same as Cmd/Ctrl+K.
    if (event.key === '/') {
      event.preventDefault();
      focusSearch();
      return;
    }
    if (event.key === 'g') {
      gotoArmed = true;
      gotoTimer = setTimeout(() => (gotoArmed = false), 1200);
      return;
    }
    if (event.key === 'G') {
      event.preventDefault();
      if (isArticle()) window.scrollTo({ top: document.body.scrollHeight, behavior: 'instant' });
      else reveal(selection.edge('last', here));
      return;
    }
    // The arrows walk a list wherever j and k would: nobody should have to know vim to read the
    // feed. An article page keeps them for scrolling, which is what reading it needs, and a list
    // with nothing in it leaves them to the browser.
    if (key === 'ArrowDown' || key === 'ArrowUp') {
      if (!isArticle() && reveal(selection.move(key === 'ArrowDown' ? 1 : -1, here))) event.preventDefault();
      return;
    }
    // Case carries meaning from here on: the upper-case letters belong to the external links
    // below, so only the letter that was actually typed may claim one of these.
    if (event.key === 'j' || event.key === 'k') {
      const direction = event.key === 'j' ? 1 : -1;
      if (isArticle()) {
        const view = page.model?.page;
        const neighbour = view?.view === 'article' ? (direction === 1 ? view.data.next : view.data.previous) : null;
        event.preventDefault();
        // Stepping past either end of the archive returns to the feed rather than stopping dead.
        void navigation.go(new URL(neighbour ? sitePath(neighbour.url) : '', page.root).href);
        return;
      }
      if (reveal(selection.move(direction, here))) event.preventDefault();
      return;
    }
    if ((event.key === 'o' || event.key === 'Enter') && !isArticle()) {
      const link = selectedLink(selection.current);
      if (link) {
        event.preventDefault();
        link.click();
      }
      return;
    }
    // Upper-case single keys open the original or a discussion.
    if (event.key.length === 1 && event.key === event.key.toUpperCase()) {
      const target = externalTarget(event.key, subject(), page.model?.site.discussions ?? []);
      if (target) {
        event.preventDefault();
        window.open(target, '_blank', 'noopener,noreferrer');
      }
    }
  });
}
