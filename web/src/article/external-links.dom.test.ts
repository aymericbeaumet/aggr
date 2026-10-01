import { describe, expect, it } from 'vitest';
import { markExternalLinks } from './external-links';

const SITE = 'http://localhost/reads/';

function body(html: string): HTMLElement {
  const element = document.createElement('div');
  element.innerHTML = html;
  return element;
}

describe('markExternalLinks', () => {
  it('asks a browser tab for a new tab on links leaving the site, and leaves the rest alone', () => {
    const root = body(
      '<a id="out" href="https://example.org/paper">the paper</a>' +
        '<a id="in" href="http://localhost/reads/items/x/">inside</a>' +
        '<a id="mail" href="mailto:a@b.c">mail</a>' +
        '<a id="hash" href="#fn-1">1</a>',
    );
    markExternalLinks(root, SITE, false);
    const out = root.querySelector<HTMLAnchorElement>('#out');
    expect(out?.target).toBe('_blank');
    expect(out?.rel).toContain('noopener');
    expect(out?.rel).toContain('noreferrer');
    expect(out?.getAttribute('aria-label')).toBe('the paper, opens in a new tab');
    for (const id of ['in', 'mail', 'hash']) {
      const link = root.querySelector<HTMLAnchorElement>(`#${id}`);
      expect(link?.hasAttribute('target')).toBe(false);
      expect(link?.hasAttribute('aria-label')).toBe(false);
    }
  });

  it('hands links to the platform in an installed app, keeping the label it already had', () => {
    const root = body('<a href="https://example.org/" target="_blank" rel="external noopener noreferrer" aria-label="Example, opens in a new tab">x</a>');
    markExternalLinks(root, SITE, true);
    const link = root.querySelector('a');
    expect(link?.hasAttribute('target')).toBe(false);
    expect(link?.getAttribute('aria-label')).toBe('Example, external site');
    expect(link?.rel).toBe('external noopener noreferrer');
    markExternalLinks(root, SITE, false);
    expect(link?.getAttribute('aria-label')).toBe('Example, opens in a new tab');
    expect(link?.target).toBe('_blank');
  });
});
