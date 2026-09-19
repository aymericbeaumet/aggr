import { expect, it, vi } from 'vitest';
import { createNavigation, enqueuePrefetch, keyboardObscuresNavigation, prefetchKey } from './navigation';

it('canonicalizes the address, saved position key and Swup history together', () => {
  const replaceState = vi.fn();
  const swup = { location: new URL('https://reader.test/nested/'), navigate: vi.fn() };
  const navigation = createNavigation({
    location: { href: 'https://reader.test/nested/', assign: vi.fn() },
    history: { state: { source: 'swup', url: '/nested/', index: 4 }, replaceState },
    base: () => 'https://reader.test/nested/', swup: () => swup,
    releaseReady: () => false, beforeNavigate: vi.fn(), reselectCurrent: vi.fn()
  });
  navigation.replaceLocation('https://reader.test/nested/?q=rust');
  expect(swup.location.href).toBe('https://reader.test/nested/?q=rust');
  expect(navigation.currentURL()).toBe(swup.location.href);
  expect(replaceState.mock.calls[0][0]).toMatchObject({ source: 'swup', index: 4, url: '/nested/?q=rust' });
});

it('uses Swup normally and full navigation only when a release is pending', () => {
  let ready = false;
  const assign = vi.fn(), navigate = vi.fn(), save = vi.fn();
  const navigation = createNavigation({ location: { href: 'https://reader.test/base/', assign },
    history: { state: null, replaceState: vi.fn() }, base: () => 'https://reader.test/base/',
    swup: () => ({ location: new URL('https://reader.test/base/'), navigate }),
    releaseReady: () => ready, beforeNavigate: save, reselectCurrent: vi.fn() });
  navigation.navigate('items/one/');
  expect(navigate).toHaveBeenCalledWith('https://reader.test/base/items/one/');
  ready = true;
  navigation.navigate('items/two/');
  expect(assign).toHaveBeenCalledWith('https://reader.test/base/items/two/');
  expect(save).toHaveBeenCalledTimes(2);
});

it('promotes an already queued intent and bounds speculation without duplicate requests', () => {
  const queue = ['/feed/', '/categories/', '/sources/', '/tags/'];
  expect(enqueuePrefetch(queue, '/tags/', true, 4)).toEqual(['/tags/', '/feed/', '/categories/', '/sources/']);
  expect(enqueuePrefetch(queue, '/item/', true, 4)).toEqual(['/item/', '/feed/', '/categories/', '/sources/']);
  expect(enqueuePrefetch(queue, '/item/', false, 4)).toEqual(['/feed/', '/categories/', '/sources/', '/item/']);
  expect(enqueuePrefetch(queue, '/tags/', false, 4)).toEqual(queue);
  expect(queue).toHaveLength(4);
});

it('hides bottom controls for an editing keyboard, not browser chrome or pinch zoom', () => {
  expect(keyboardObscuresNavigation(true, 844, 500, 1)).toBe(true);
  expect(keyboardObscuresNavigation(false, 844, 500, 1)).toBe(false);
  expect(keyboardObscuresNavigation(true, 844, 740, 1)).toBe(false);
  expect(keyboardObscuresNavigation(true, 844, 500, 2)).toBe(false);
});

it('keys speculative loads by page, ignoring fragments, the search focus flag, other sites and the current page', () => {
  const base = 'https://reader.test/nested/';
  const current = { pathname: '/nested/', search: '?q=rust' };
  expect(prefetchKey('items/one/#top', base, base, current)).toBe('/nested/items/one/');
  expect(prefetchKey(base + '?focus-search=1&q=svelte', base, base, current)).toBe('/nested/?q=svelte');
  expect(prefetchKey(base + '?q=rust#list', base, base, current)).toBeNull();
  expect(prefetchKey('items/one/index.html', base, base, current)).toBeNull();
  expect(prefetchKey('https://elsewhere.test/nested/items/one/', base, base, current)).toBeNull();
  expect(prefetchKey('/other/items/one/', base, base, current)).toBeNull();
  expect(prefetchKey('http://[bad', base, base, current)).toBeNull();
});
