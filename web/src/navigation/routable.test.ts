import { describe, expect, it } from 'vitest';
import { address, routable } from './routable';

const root = new URL('https://x.test/reads/');

describe('routable', () => {
  it('accepts pages under the site root', () => {
    expect(routable(new URL('https://x.test/reads/'), root)).toBe(true);
    expect(routable(new URL('https://x.test/reads/items/blog/a/'), root)).toBe(true);
    expect(routable(new URL('https://x.test/reads/404.html'), root)).toBe(true);
    expect(routable(new URL('https://x.test/reads/?q=rust#top'), root)).toBe(true);
  });

  it('rejects files, other sites and paths outside the root', () => {
    expect(routable(new URL('https://x.test/reads/atom.xml'), root)).toBe(false);
    expect(routable(new URL('https://x.test/reads/items/a.md'), root)).toBe(false);
    expect(routable(new URL('https://x.test/reads/aggr.toml'), root)).toBe(false);
    expect(routable(new URL('https://x.test/other/'), root)).toBe(false);
    expect(routable(new URL('https://y.test/reads/'), root)).toBe(false);
    expect(routable(new URL('http://x.test/reads/'), root)).toBe(false);
  });
});

describe('address', () => {
  it('drops the fragment and resolves relative references', () => {
    expect(address('https://x.test/reads/a/#x')).toBe('https://x.test/reads/a/');
    expect(address('../b/', 'https://x.test/reads/a/')).toBe('https://x.test/reads/b/');
    expect(address('https://x.test/reads/?q=1#x')).toBe('https://x.test/reads/?q=1');
  });
});
