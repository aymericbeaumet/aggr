import { describe, expect, it } from 'vitest';
import { markCurrentTab, markTabForKind, tabUnder, type TabBox } from './tabs';

const bar: TabBox = { left: 0, top: 700, width: 300, height: 68 };

function link(route: string): HTMLAnchorElement {
  const anchor = document.createElement('a');
  anchor.href = `https://x.test/${route}`;
  anchor.dataset.route = route;
  return anchor;
}

describe('tabUnder', () => {
  const links = [link(''), link('browse/'), link('preferences/')];

  it('maps the whole bar, including its padding, onto a column', () => {
    expect(tabUnder(links, bar, 0, 700)).toBe(links[0]);
    expect(tabUnder(links, bar, 10, 760)).toBe(links[0]);
    expect(tabUnder(links, bar, 100, 732)).toBe(links[1]);
    expect(tabUnder(links, bar, 299, 767)).toBe(links[2]);
  });

  it('ignores a point outside the bar and an empty or zero-sized bar', () => {
    expect(tabUnder(links, bar, -1, 730)).toBeNull();
    expect(tabUnder(links, bar, 300, 730)).toBeNull();
    expect(tabUnder(links, bar, 40, 699)).toBeNull();
    expect(tabUnder(links, bar, 40, 768)).toBeNull();
    expect(tabUnder([], bar, 40, 730)).toBeNull();
    expect(tabUnder(links, { ...bar, width: 0 }, 0, 730)).toBeNull();
  });

  it('counts columns from the right when the bar is right to left', () => {
    expect(tabUnder(links, bar, 10, 730, true)).toBe(links[2]);
    expect(tabUnder(links, bar, 290, 730, true)).toBe(links[0]);
  });
});

describe('tab marks', () => {
  function tabs(): HTMLElement {
    const root = document.createElement('div');
    root.innerHTML = `
      <nav data-site-navigation>
        <a class="brand" data-route="" href="/"></a>
        <a data-route="" data-kinds="river" href="/"></a>
        <a data-route="browse/" data-kinds="browse categories category" href="/browse/"></a>
      </nav>
      <nav class="mobile-tabs" data-site-navigation>
        <a data-route="" data-kinds="river" href="/"></a>
        <a data-route="browse/" data-kinds="browse categories category" href="/browse/"></a>
      </nav>`;
    document.body.append(root);
    return root;
  }

  it('marks one route current on the header and the tab bar together', () => {
    const root = tabs();
    markCurrentTab(root, '');
    expect(root.querySelector('.brand')?.hasAttribute('aria-current')).toBe(false);
    markCurrentTab(root, 'browse/');
    const current = [...root.querySelectorAll('[aria-current]')].map((node) => node.getAttribute('data-route'));
    expect(current).toEqual(['browse/', 'browse/']);
    markTabForKind(root, 'categories');
    expect([...root.querySelectorAll('[aria-current]')].map((node) => node.getAttribute('data-route'))).toEqual([
      'browse/',
      'browse/',
    ]);
    markTabForKind(root, 'item');
    expect(root.querySelector('[aria-current]')).toBeNull();
    root.remove();
  });
});
