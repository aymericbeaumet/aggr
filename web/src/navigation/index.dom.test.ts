import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const model = (path: string, title = 'Reader') => ({
  base: '/repo/', path, kind: path ? 'browse' : 'river', title,
  site: { title: 'Reader', language: 'en', pwa: false, discussions: [], entries: [], config_url: null, assets: { app: null, css: null, imports: [] } },
  build: { version: '1', app: 'fixture', content: 'fixture' },
  page: { view: 'not_found' },
});

function response(url: string, path = 'article/', content = 'Arrived content'): Response {
  const html = `<html><head><title>Arrived</title><script id="aggr-page" type="application/json">${JSON.stringify(model(path))}</script></head><body><main id="content" tabindex="-1">${content}</main></body></html>`;
  const result = new Response(html, { headers: { 'content-type': 'text/html' } });
  Object.defineProperty(result, 'url', { value: url });
  return result;
}

let navigation: typeof import('./index')['navigation'];
let scope: typeof import('./scope')['pageScope'];
let currentPage: typeof import('../state/page.svelte')['page'];
let documentEvents: ReturnType<typeof vi.spyOn>;
let windowEvents: ReturnType<typeof vi.spyOn>;
let connection: PropertyDescriptor | undefined;

beforeEach(async () => {
  vi.resetModules();
  vi.stubGlobal('fetch', vi.fn());
  history.replaceState({}, '', '/repo/');
  document.head.innerHTML = '<title>Current</title>';
  document.body.innerHTML = '<main id="content" tabindex="-1">Current content</main>';
  documentEvents = vi.spyOn(document, 'addEventListener');
  windowEvents = vi.spyOn(window, 'addEventListener');
  vi.spyOn(window, 'scrollTo').mockImplementation(() => {});
  vi.spyOn(window, 'requestAnimationFrame').mockImplementation(() => 0);
  vi.spyOn(console, 'error').mockImplementation(() => {});
  connection = Object.getOwnPropertyDescriptor(navigator, 'connection');
  Object.defineProperty(navigator, 'connection', { configurable: true, value: { saveData: false } });
  currentPage = (await import('../state/page.svelte')).page;
  currentPage.root = new URL('/repo/', location.href).href;
  currentPage.show({ model: JSON.parse(JSON.stringify(model(''))), bodyHtml: '', href: location.href });
  scope = (await import('./scope')).pageScope;
  navigation = (await import('./index')).navigation;
  navigation.install();
});

afterEach(() => {
  vi.useRealTimers();
  document.documentElement.removeAttribute('data-navigating');
  scope.abort();
  for (const [type, listener, options] of documentEvents.mock.calls) document.removeEventListener(type, listener, options);
  for (const [type, listener, options] of windowEvents.mock.calls) window.removeEventListener(type, listener, options);
  if (connection) Object.defineProperty(navigator, 'connection', connection);
  else Reflect.deleteProperty(navigator, 'connection');
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('page navigation', () => {
  it('does not download pages before link intent', () => {
    expect(fetch).not.toHaveBeenCalled();
  });
  it('prefetches a focused archive link and leaves external links alone', () => {
    const url = new URL('/repo/article/', location.href).href;
    const request = vi.fn(async (_input: RequestInfo | URL) => response(url));
    vi.stubGlobal('fetch', request);
    document.body.insertAdjacentHTML('beforeend', '<a id="inside" href="/repo/article/">Article</a><a id="outside" href="https://publisher.test/">Publisher</a>');
    document.getElementById('outside')?.focus();
    expect(request).not.toHaveBeenCalled();
    document.getElementById('inside')?.focus();
    expect(request).toHaveBeenCalledTimes(1);
    expect(request.mock.calls[0][0]).toBe(url);
  });
  it('bounds simultaneous intent prefetches', () => {
    const request = vi.fn(() => new Promise<Response>(() => {}));
    vi.stubGlobal('fetch', request);
    for (const path of ['one/', 'two/', 'three/']) navigation.prefetch(`/repo/${path}`);
    expect(request).toHaveBeenCalledTimes(2);
  });
  it.each([{ saveData: true }, { effectiveType: 'slow-2g' }, { effectiveType: '2g' }])(
    'leaves constrained connections to requested navigation: %j', async (connection) => {
      Object.defineProperty(navigator, 'connection', { configurable: true, value: connection });
      vi.stubGlobal('fetch', vi.fn(async () => response(new URL('/repo/article/', location.href).href)));
      navigation.prefetch('/repo/article/');
      expect(fetch).not.toHaveBeenCalled();
      await navigation.go('/repo/article/');
      expect(fetch).toHaveBeenCalledOnce();
      expect(vi.mocked(fetch).mock.calls[0][1]?.priority).toBe('high');
    },
  );
  it('fetches speculative pages at low priority', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => response(new URL('/repo/article/', location.href).href)));
    navigation.prefetch('/repo/article/');
    await navigation.go('/repo/article/');
    expect(vi.mocked(fetch).mock.calls[0][1]?.priority).toBe('low');
  });
  it('ends a stalled response body and releases its prefetch slot', async () => {
    vi.useFakeTimers();
    const signals: AbortSignal[] = [];
    vi.stubGlobal('fetch', vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      const signal = init?.signal as AbortSignal;
      signals.push(signal);
      return {
        ok: true, headers: new Headers({ 'content-type': 'text/html' }),
        text: () => new Promise<string>((_resolve, reject) => {
          signal.addEventListener('abort', () => reject(new DOMException('Timed out', 'AbortError')), { once: true });
        }),
      };
    }));
    navigation.prefetch('/repo/one/');
    navigation.prefetch('/repo/two/');
    await vi.advanceTimersByTimeAsync(15_000);
    expect(signals.every((signal) => signal.aborted)).toBe(true);
    navigation.prefetch('/repo/three/');
    expect(fetch).toHaveBeenCalledTimes(3);
    await vi.advanceTimersByTimeAsync(15_000);
  });
  it('announces a pending page without moving the current content', async () => {
    vi.useFakeTimers();
    let resolve!: (response: Response) => void;
    vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>((done) => { resolve = done; })));
    const pending = navigation.go('/repo/article/');
    expect(document.getElementById('content')?.getAttribute('aria-busy')).toBe('true');
    expect(document.documentElement.hasAttribute('data-navigating')).toBe(false);
    await vi.advanceTimersByTimeAsync(150);
    expect(document.documentElement.hasAttribute('data-navigating')).toBe(true);
    resolve(response(new URL('/repo/article/', location.href).href));
    await pending;
    expect(document.getElementById('content')?.hasAttribute('aria-busy')).toBe(false);
    expect(document.documentElement.hasAttribute('data-navigating')).toBe(false);
  });
  it('loads typed page state and only its opaque body payload', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => response(new URL('/repo/nested/', location.href).href, 'nested/', '<div data-article-body><p id="note">Note</p></div>')));
    await navigation.go('/repo/nested/');
    expect(currentPage.model?.path).toBe('nested/');
    expect(currentPage.bodyHtml).toBe('<p id="note">Note</p>');
  });
  it('shares a prefetched document between different article fragments', async () => {
    const url = new URL('/repo/article/', location.href).href;
    const fetch = vi.fn(async (_input: RequestInfo | URL) => response(url));
    vi.stubGlobal('fetch', fetch);
    navigation.prefetch(`${url}#first`);
    await navigation.go(`${url}#second`);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(fetch.mock.calls[0][0]).toBe(url);
    expect(location.hash).toBe('#second');
    expect(navigation.phase).toBe('settled');
  });

  it.each(['https://external.test/article/', '/outside/article/'])('leaves the current page mounted for a redirect outside the archive: %s', async (destination) => {
    vi.stubGlobal('fetch', vi.fn(async () => response(new URL(destination, location.href).href)));
    const original = currentPage.current;
    const originalSignal = scope.signal;
    await expect(navigation.go('/repo/article/')).resolves.toBeUndefined();
    expect(originalSignal.aborted).toBe(false);
    expect(currentPage.current).toBe(original);
    expect(location.pathname).toBe('/repo/');
    expect(navigation.pending).toBe(false);
  });

  it('keeps redirects within the archive and the requested article fragment', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => response(new URL('/repo/renamed/', location.href).href)));
    const originalSignal = scope.signal;
    await navigation.go('/repo/article/#section');
    expect(location.pathname).toBe('/repo/renamed/');
    expect(location.hash).toBe('#section');
    expect(currentPage.href).toBe(location.href);
    // Component disposal owns page lifecycle cleanup.
    expect(navigation.phase).toBe('settled');
  });

  it.each(['pushState', 'replaceState'] as const)('preserves the current page when the browser refuses %s', async (method) => {
    vi.stubGlobal('fetch', vi.fn(async () => response(new URL('/repo/article/', location.href).href)));
    vi.spyOn(history, method).mockImplementation(() => { throw new DOMException('history unavailable', 'SecurityError'); });
    const originalSignal = scope.signal;
    const original = currentPage.current;
    await expect(navigation.go('/repo/article/', { replace: method === 'replaceState' })).resolves.toBeUndefined();
    expect(originalSignal.aborted).toBe(false);
    expect(currentPage.current).toBe(original);
    expect(navigation.pending).toBe(false);
  });
});

function mobileTabs(): { bar: HTMLElement; home: HTMLAnchorElement } {
  const bar = document.createElement('nav');
  bar.className = 'mobile-tabs';
  bar.setAttribute('data-site-navigation', '');
  bar.innerHTML = '<a data-route="" data-kinds="river" href="/repo/">Feed</a><a data-route="browse/" data-kinds="browse" href="/repo/browse/">Browse</a><a data-route="preferences/" data-kinds="preferences" href="/repo/preferences/">Preferences</a>';
  bar.getBoundingClientRect = () => new DOMRect(0, 700, 300, 68);
  document.body.append(bar);
  return { bar, home: bar.querySelector('a')! };
}

const touch = (x: number, y = 730, identifier = 1) => ({ clientX: x, clientY: y, identifier });

function touches(target: EventTarget, name: string, active: ReturnType<typeof touch>[], changed: ReturnType<typeof touch>[] = active): Event {
  const event = new Event(name, { bubbles: true, cancelable: true });
  Object.defineProperties(event, { touches: { value: active }, changedTouches: { value: changed } });
  target.dispatchEvent(event);
  return event;
}

describe('mobile tab gestures', () => {
  it('releases contact feedback after a touch whose pointer was canceled by scrolling', async () => {
    (await import('../app/platform')).installTapFeedback();
    const { bar, home } = mobileTabs();
    const click = vi.spyOn(home, 'click').mockImplementation(() => {});
    touches(bar, 'touchstart', [touch(20)]);
    document.dispatchEvent(new Event('pointercancel'));
    touches(bar, 'touchmove', [touch(25)]);
    expect(home.hasAttribute('data-pressed')).toBe(true);
    const release = touches(bar, 'touchend', [], [touch(25)]);
    expect(release.defaultPrevented).toBe(true);
    expect(click).toHaveBeenCalledOnce();
    expect(home.hasAttribute('data-pressed')).toBe(false);
  });

  it('cancels a drag off the bar without activating a tab', () => {
    const { bar, home } = mobileTabs();
    const click = vi.spyOn(home, 'click').mockImplementation(() => {});
    touches(bar, 'touchstart', [touch(20)]);
    touches(bar, 'touchmove', [touch(20, 680)]);
    const release = touches(bar, 'touchend', [], [touch(20, 680)]);
    expect(release.defaultPrevented).toBe(true);
    expect(click).not.toHaveBeenCalled();
    expect(bar.querySelector('[data-pressed]')).toBeNull();
  });

  it('lets a second finger cancel activation and contact feedback', () => {
    const { bar, home } = mobileTabs();
    const click = vi.spyOn(home, 'click').mockImplementation(() => {});
    touches(bar, 'touchstart', [touch(20)]);
    touches(document.body, 'touchstart', [touch(20), touch(100, 400, 2)]);
    touches(bar, 'touchend', [], [touch(20), touch(100, 400, 2)]);
    expect(click).not.toHaveBeenCalled();
    expect(bar.querySelector('[data-pressed]')).toBeNull();
  });
});
