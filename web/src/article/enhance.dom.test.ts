/// <reference types="node" />
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import type { ClientArticle } from '../generated/ClientArticle';
import type { ClientPage } from '../generated/ClientPage';
import Article from '../pages/Article.svelte';
import { page } from '../state/page.svelte';

type Fixture = { component: string; props: { page: ClientPage } };

// Under jsdom `import.meta.url` is the page's address, so the fixture is found from the root.
const fixture = JSON.parse(readFileSync(resolve(process.cwd(), 'test/parity/page-article.json'), 'utf8')) as Fixture;

function article(): ClientArticle {
  const view = fixture.props.page.page;
  if (view.view !== 'article') throw new Error('not an article fixture');
  return view.data;
}

const BODY = `<figure class="article-lead media-frame"><img src="lead.jpg" alt=""></figure>
<div class="body e-content">
<h2 id="section">Section<a class="heading-anchor" href="#section" aria-label="Link to this section">#</a></h2>
<p>Read <a href="https://example.org/">elsewhere</a> or <a href="#section">here</a>.</p>
</div>`;

function content(): HTMLElement {
  const region = document.createElement('div');
  region.setAttribute('data-article-content', '');
  region.innerHTML = BODY;
  return region;
}

beforeEach(() => {
  page.root = 'http://localhost/';
  // A phone: the swipe mark only goes on where a finger can use it.
  Object.defineProperty(navigator, 'maxTouchPoints', { value: 5, configurable: true });
});

afterEach(() => {
  document.body.innerHTML = '';
  Object.defineProperty(navigator, 'maxTouchPoints', { value: 0, configurable: true });
});

describe('Article', () => {
  it('enhances the adopted region after moving it in, and ends it all with the page', () => {
    const target = document.createElement('div');
    document.body.append(target);
    const region = content();
    const heading = region.querySelector('h2');
    const app = mount(Article, { target, props: { page: fixture.props.page, article: article(), content: region } });
    flushSync();
    const root = target.querySelector('article.item');
    expect(root?.hasAttribute('data-swipe-navigation')).toBe(true);
    // The very same nodes, not a copy, and the enhancements found them there.
    expect(target.querySelector('[data-article-content] h2')).toBe(heading);
    const external = target.querySelector<HTMLAnchorElement>('a[href="https://example.org/"]');
    expect(external?.target).toBe('_blank');
    expect(external?.getAttribute('aria-label')).toBe('elsewhere, opens in a new tab');
    expect(target.querySelector<HTMLAnchorElement>('a[href="#section"]:not(.heading-anchor)')?.hasAttribute('target')).toBe(false);
    const toolbar = document.querySelector<HTMLElement>('.selection-share');
    expect(toolbar).not.toBeNull();
    expect(toolbar?.hidden).toBe(true);
    unmount(app);
    expect(document.querySelector('.selection-share')).toBeNull();
    expect(root?.hasAttribute('data-swipe-navigation')).toBe(false);
  });

  it('renders the same chrome without content, as the server does', () => {
    const target = document.createElement('div');
    document.body.append(target);
    const app = mount(Article, { target, props: { page: fixture.props.page, article: article(), content: null } });
    flushSync();
    expect(target.querySelector('[data-article-content]')?.childElementCount).toBe(0);
    expect(document.querySelector('.selection-share')).toBeNull();
    unmount(app);
  });
});
