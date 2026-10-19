import { flushSync, hydrate, unmount } from 'svelte';
import { extract } from '../model/extract';
import { readModel } from '../model/page';
import { navigation } from '../navigation';
import { installFrameFocus } from '../media/frame-focus';
import { installShortcuts } from '../shortcuts';
import { installShortcutHelp } from '../shortcuts/help';
import { startClock } from '../state/clock.svelte';
import { page, type Shown } from '../state/page.svelte';
import { loadPreferences } from '../state/preferences.svelte';
import { selection } from '../state/selection.svelte';
import App from './App.svelte';
import {
  installKeyboardInset,
  installPictureStates,
  installShiftHover,
  installTapFeedback,
  markPlatform,
} from './platform';
import { pageArrived, startReader } from './reader';

const persistent = new WeakMap<Document, () => void>();

/** Hydrate the rendered tree without replacing its root or article nodes. */
export function mountApp(doc: Document = document): Shown | null {
  const extracted = extract(doc);
  const existing = doc.getElementById('app');
  if (!extracted || !existing) return null;
  persistent.get(doc)?.();
  const { model, bodyHtml } = extracted;
  page.root = new URL(model.base, doc.baseURI).href;
  const shown: Shown = { model, bodyHtml, href: doc.location.href };
  page.show(shown);
  const app = hydrate(App, {
    target: existing,
    props: {
      get page() { return page.model ?? model; },
      get bodyHtml() { return page.bodyHtml; },
    },
  });
  persistent.set(doc, () => { void unmount(app); });
  flushSync();
  return shown;
}

/** Never let one broken enhancement take the rest of the page down with it. */
function safely(label: string, run: () => void): void {
  try {
    run();
  } catch (error) {
    console.error(`aggr: ${label}`, error);
  }
}

/** The whole reader, once the page may be seen. */
export function boot(): void {
  const model = readModel(document.getElementById('aggr-page')?.textContent);
  if (!model) return;
  const root = new URL(model.base, document.baseURI);
  safely('preferences', () => loadPreferences());
  safely('clock', () => startClock());
  safely('selection', () => selection.configure(root.pathname, sessionStorage));
  if (!mountApp()) return;
  safely('platform', markPlatform);
  safely('shift-hover', installShiftHover);
  safely('tap-feedback', installTapFeedback);
  safely('keyboard-inset', installKeyboardInset);
  safely('pictures', installPictureStates);
  safely('shortcut-help', installShortcutHelp);
  safely('shortcuts', installShortcuts);
  safely('frame-focus', installFrameFocus);
  // Deployment polling, the new-entry badges, pull-to-refresh and the service worker.
  safely('reader', () => { startReader(); pageArrived(); });
  safely('navigation', () => navigation.install());
  // One readiness signal, for styles that only apply once enhancement is in place and for the
  // browser contract suite.
  flushSync();
  document.documentElement.dataset.aggrReady = 'true';
}
