import { describe, expect, it } from 'vitest';
import { linkDestination, linkIn } from './routable';

const root = new URL('https://x.test/reads/');

function anchor(attributes: Record<string, string>): HTMLAnchorElement {
  const link = document.createElement('a');
  for (const [name, value] of Object.entries(attributes)) link.setAttribute(name, value);
  return link;
}

describe('linkDestination', () => {
  it('follows ordinary same-site page links only', () => {
    expect(linkDestination(anchor({ href: 'https://x.test/reads/browse/' }), root)?.href).toBe('https://x.test/reads/browse/');
    expect(linkDestination(anchor({ href: 'https://x.test/reads/browse/', download: '' }), root)).toBeNull();
    expect(linkDestination(anchor({ href: 'https://x.test/reads/browse/', target: '_blank' }), root)).toBeNull();
    expect(linkDestination(anchor({ href: 'https://x.test/reads/browse/', target: '_self' }), root)).not.toBeNull();
    expect(linkDestination(anchor({ href: 'https://x.test/reads/browse/', rel: 'external noopener' }), root)).toBeNull();
    expect(linkDestination(anchor({ href: 'https://x.test/reads/feed.json' }), root)).toBeNull();
    expect(linkDestination(anchor({ href: 'https://other.test/' }), root)).toBeNull();
  });
});

describe('linkIn', () => {
  it('finds the link an event landed inside', () => {
    const link = anchor({ href: 'https://x.test/reads/items/a/' });
    const inner = document.createElement('span');
    link.append(inner);
    document.body.append(link);
    const hits: Array<ReturnType<typeof linkIn>> = [];
    inner.addEventListener('click', (event) => hits.push(linkIn(event, root)));
    inner.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
    expect(hits[0]?.link).toBe(link);
    expect(hits[0]?.url.href).toBe('https://x.test/reads/items/a/');
    link.remove();
  });
});
