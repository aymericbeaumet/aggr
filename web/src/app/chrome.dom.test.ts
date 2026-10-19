import { flushSync } from 'svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { ClientPage } from '../generated/ClientPage';
import { pull } from '../offline/pull-refresh.svelte';
import { announcer } from '../state/announcer.svelte';
import { connection } from '../state/connection.svelte';
import { page } from '../state/page.svelte';
import { tabIntent } from '../state/tabIntent.svelte';
import { versions } from '../state/versions.svelte';
import { installChrome } from './chrome.svelte';
import { refreshToUpdate, retryConnection } from './reader';

vi.mock('./reader', () => ({ refreshToUpdate: vi.fn(), retryConnection: vi.fn() }));

let stop: (() => void) | undefined;
const element = (id: string) => document.getElementById(id) as HTMLElement;
const show = (kind: string) => page.show({
  model: { kind, build: { app: 'current', content: 'content' } } as ClientPage,
  content: null,
  href: location.href,
});

beforeEach(() => {
  vi.clearAllMocks();
  history.replaceState(null, '', '/reader/');
  document.head.innerHTML = '<link rel="manifest" href="./manifest.webmanifest">';
  connection.online = true;
  versions.availableApp = '';
  tabIntent.route = null;
  announcer.message = '';
  pull.phase = 'idle';
  show('river');
  document.body.innerHTML = `<nav data-site-navigation>
    <a id="feed" href="./" data-kinds="river" data-route="">Custom feed label</a>
    <a id="browse" href="./browse/" data-kinds="browse source" data-route="browse/">Browse</a>
  </nav><div id="reader-announcement"></div><div id="pull-refresh" hidden></div>
  <div id="connection-status" hidden><span id="connection-status-message"></span>
  <button id="connection-retry" hidden>Retry</button><button id="pwa-refresh" hidden>Refresh</button></div>`;
});

afterEach(() => { stop?.(); stop = undefined; });

it('preserves template nodes and labels while tracking navigation and pressed tabs', () => {
  const feed = element('feed');
  stop = installChrome();
  flushSync();
  expect(element('feed')).toBe(feed);
  expect(feed.textContent).toBe('Custom feed label');
  expect(feed.getAttribute('aria-current')).toBe('page');
  tabIntent.route = 'browse/';
  flushSync();
  expect(feed.hasAttribute('aria-current')).toBe(false);
  expect(element('browse').getAttribute('aria-current')).toBe('page');
  tabIntent.route = null;
  show('source');
  flushSync();
  expect(element('browse').getAttribute('aria-current')).toBe('page');
});

it('updates existing status regions and releases their handlers on teardown', () => {
  stop = installChrome();
  connection.online = false;
  announcer.message = 'New page';
  pull.phase = 'armed';
  flushSync();
  expect(element('connection-status').hidden).toBe(false);
  expect(element('connection-retry').hidden).toBe(false);
  expect(element('reader-announcement').textContent).toBe('New page');
  expect(element('pull-refresh').textContent).toBe('Release to refresh');
  element('connection-retry').click();
  expect(retryConnection).toHaveBeenCalledTimes(1);
  versions.availableApp = 'next';
  flushSync();
  expect(element('connection-retry').hidden).toBe(true);
  expect(element('pwa-refresh').hidden).toBe(false);
  expect(document.documentElement.dataset.updateState).toBe('ready');
  element('pwa-refresh').click();
  expect(refreshToUpdate).toHaveBeenCalledTimes(1);
  stop();
  stop = undefined;
  element('pwa-refresh').click();
  expect(refreshToUpdate).toHaveBeenCalledTimes(1);
});

it('keeps persistent navigation links rooted after navigating to a nested article', () => {
  stop = installChrome();
  history.replaceState(null, '', '/reader/items/story/');
  expect(new URL((element('feed') as HTMLAnchorElement).href).pathname).toBe('/reader/');
  expect(new URL((element('browse') as HTMLAnchorElement).href).pathname).toBe('/reader/browse/');
  expect(new URL(document.querySelector<HTMLLinkElement>('link[rel="manifest"]')!.href).pathname).toBe('/reader/manifest.webmanifest');
});

it('does not restart loaded stylesheets or preloads while rooting the shell', () => {
  document.head.insertAdjacentHTML('beforeend', '<link rel="stylesheet" href="./assets/style.css"><link rel="modulepreload" href="./assets/app.js">');
  stop = installChrome();
  expect(document.querySelector('link[rel="stylesheet"]')?.getAttribute('href')).toBe('./assets/style.css');
  expect(document.querySelector('link[rel="modulepreload"]')?.getAttribute('href')).toBe('./assets/app.js');
});
