import { flushSync, mount } from 'svelte';
import { contentElement } from '../model/extract';
import { readModel } from '../model/page';
import { navigation } from '../navigation';
import { installShortcuts } from '../shortcuts';
import { installShortcutHelp } from '../shortcuts/help';
import { installSearchIntent } from '../shortcuts/search';
import { installReadingHeader } from '../article/readingHeader';
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
  settlePictures,
} from './platform';
import { pageArrived, startReader } from './reader';

/**
 * Take the rendered page over: read its model, capture the region a view adopts, mount `App`
 * into a detached `<div id="app">`, swap it over the rendered one in the same task and flush,
 * so nothing paints in between. Returns what is shown, or null when the page has no usable
 * model and stays static.
 */
export function mountApp(doc: Document = document): Shown | null {
  const model = readModel(doc.getElementById('aggr-page')?.textContent);
  const existing = doc.getElementById('app');
  if (!model || !existing) return null;
  page.root = new URL(model.base, doc.baseURI).href;
  page.icon = doc.querySelector('link[rel="icon"]')?.getAttribute('href') ?? null;
  if (page.icon) page.icon = new URL(page.icon, doc.baseURI).href;
  const content = contentElement(existing, model);
  const shown: Shown = { model, content, href: doc.location.href };
  page.show(shown);
  const target = doc.createElement('div');
  target.id = 'app';
  mount(App, {
    target,
    props: {
      get page() {
        return page.model ?? model;
      },
      get content() {
        return page.content;
      },
    },
  });
  existing.replaceWith(target);
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
  // Deployment polling, the new-entry badges, pull-to-refresh and the service worker.
  safely('reader', () => startReader());
  // Everything that belongs to the page on screen, run when it arrives: now, and again each time
  // another page is swapped in. What it sets up ends with the page's signal.
  safely('navigation', () =>
    navigation.install([
      () => settlePictures(),
      (signal) => installReadingHeader(signal),
      (signal) => installSearchIntent(signal),
      (signal) => navigation.speculate(signal),
      () => pageArrived(),
    ]),
  );
  // One readiness signal, for styles that only apply once enhancement is in place and for the
  // browser contract suite.
  document.documentElement.dataset.aggrReady = 'true';
}
