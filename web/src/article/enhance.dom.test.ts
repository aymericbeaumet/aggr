import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it } from "../../test/browser";
import { fixture } from '../../test/fixtures';
import Article from '../pages/Article.svelte';
import { page } from '../state/page.svelte';

afterEach(() => { document.body.innerHTML = ''; });
describe('article lifecycle', () => {
  it('enhances opaque prose and releases its listeners when the component leaves', async () => {
    const model = fixture('page-article');
    if (model.page.view !== 'article') throw new Error('article fixture');
    page.root = 'http://localhost/';
    Object.defineProperty(navigator, 'maxTouchPoints', { value: 5, configurable: true });
    const target = document.createElement('div'); document.body.append(target);
    const app = mount(Article, { target, props: { page: model, article: model.page.data,
      bodyHtml: '<h2 id="section">Section</h2><p>Read <a href="https://example.org/">elsewhere</a> or <a href="#section">here</a>.</p>' } });
    flushSync();
    const root = target.querySelector('article');
    expect(root?.hasAttribute('data-swipe-navigation')).toBe(true);
    const external = target.querySelector<HTMLAnchorElement>('a[href="https://example.org/"]');
    expect(external?.target).toBe('_blank');
    expect(external?.getAttribute('aria-label')).toBe('elsewhere, opens in a new tab');
    expect(target.querySelector('a[href="#section"]')?.hasAttribute('target')).toBe(false);
    expect(document.querySelector('.selection-share')).not.toBeNull();
    await unmount(app);
    expect(document.querySelector('.selection-share')).toBeNull();
    Object.defineProperty(navigator, 'maxTouchPoints', { value: 0, configurable: true });
  });
});
