/// <reference types="node" />
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { beforeEach, describe, expect, it } from 'vitest';
import { normalize } from '../../test/parity/normalize';
import { page } from '../state/page.svelte';
import { mountApp } from './boot';

type Fixture = { component: string; props: { page: unknown } };

// Under jsdom `import.meta.url` is the page's address, so the fixtures are found from the root.
const directory = resolve(process.cwd(), 'test/parity');
// The fixture holds what `App` renders inside the root; boot creates the root itself.
const fixture = (name: string) => ({
  props: JSON.parse(readFileSync(resolve(directory, `${name}.json`), 'utf8')) as Fixture,
  html: `<div id="app">${readFileSync(resolve(directory, `${name}.html`), 'utf8')}</div>`,
});

/** The fixture as a document: the model in the head, `#app` in the body. */
function load(name: string, body = fixture(name).html): void {
  const { props } = fixture(name);
  document.head.innerHTML = '';
  const script = document.createElement('script');
  script.id = 'aggr-page';
  script.type = 'application/json';
  script.textContent = JSON.stringify(props.props.page);
  document.head.append(script);
  document.body.innerHTML = body;
}

/**
 * Both renders in the form a live document gives them, so the comparison is about markup and
 * not about what the platform does to it: links resolve against the document (the sliced pager
 * writes absolute ones), inputs hold their value as a property, a style attribute comes back
 * re-serialized, and `<noscript>` is empty on the client, where it is inert. The cursor's own
 * mark is asserted apart.
 */
function canonical(html: string): string {
  const template = document.createElement('template');
  template.innerHTML = html;
  return normalize(canonicalNodes(template.content));
}

/** The live app, with what its inputs hold written back as attributes, then canonical. */
function live(app: Element): string {
  for (const input of app.querySelectorAll('input')) input.setAttribute('value', input.value);
  return canonical(app.outerHTML);
}

function canonicalNodes(root: DocumentFragment): string {
  for (const link of root.querySelectorAll('[href]')) {
    link.setAttribute('href', new URL(link.getAttribute('href') as string, document.baseURI).href);
  }
  for (const input of root.querySelectorAll('input')) input.setAttribute('value', input.value);
  for (const styled of root.querySelectorAll('[style]')) {
    styled.setAttribute('style', (styled.getAttribute('style') as string).replace(/;\s*$/, ''));
  }
  for (const noscript of root.querySelectorAll('noscript')) noscript.textContent = '';
  for (const row of root.querySelectorAll('.row.is-selected')) row.classList.remove('is-selected');
  return (root.firstElementChild as Element).outerHTML;
}

describe('mountApp', () => {
  beforeEach(() => {
    sessionStorage.clear();
  });

  it('replaces #app with a render that matches the static page', () => {
    load('page-river');
    const before = document.getElementById('app');
    const shown = mountApp(document);
    expect(shown?.model.kind).toBe('river');
    const app = document.getElementById('app');
    expect(app).not.toBe(before);
    expect(app?.parentNode).toBe(document.body);
    expect(live(app as Element)).toBe(canonical(fixture('page-river').html));
    expect(page.root).toBe(new URL('./', document.baseURI).href);
    // The cursor came back on the first row, as the vanilla reader restored it.
    const selected = document.querySelectorAll('.row.is-selected');
    expect(selected).toHaveLength(1);
    expect(selected[0]).toBe(document.querySelector('.rows .row'));
  });

  it('renders the scoped list and its error', () => {
    load('page-source-error');
    mountApp(document);
    expect(live(document.getElementById('app') as Element)).toBe(
      canonical(fixture('page-source-error').html),
    );
  });

  it('moves the article content into the new tree without re-parsing it', () => {
    const { html } = fixture('page-article');
    const body = html.replace('<div data-article-content=""></div>', '<div data-article-content=""><figure class="article-lead"></figure><div class="body e-content"><p>Body</p></div></div>');
    expect(body).not.toBe(html);
    load('page-article', body);
    const paragraph = document.querySelector('[data-article-content] p');
    const figure = document.querySelector('[data-article-content] figure');
    expect(paragraph).not.toBeNull();
    mountApp(document);
    const region = document.querySelector('#app [data-article-content]');
    expect(region?.children[0]).toBe(figure);
    expect(region?.querySelector('p')).toBe(paragraph);
    expect(document.querySelectorAll('[data-article-content]')).toHaveLength(1);
    expect(live(document.getElementById('app') as Element)).toBe(canonical(body));
    expect(document.querySelector('article.item')?.getAttribute('lang')).toBe('fr');
  });

  it('adopts a static page into the new main', () => {
    const { html } = fixture('page-browse');
    const body = html.replace('<main class="main" id="content" tabindex="-1"></main>', '<main class="main" id="content" tabindex="-1"><section class="page"><h1>Browse</h1></section></main>');
    load('page-browse', body);
    const section = document.querySelector('main section');
    mountApp(document);
    const main = document.querySelector('#app main');
    expect(main?.querySelector('section')).toBe(section);
    expect(live(document.getElementById('app') as Element)).toBe(canonical(body));
  });

  it('leaves a page without a model alone', () => {
    load('page-river');
    document.getElementById('aggr-page')?.remove();
    const before = document.getElementById('app');
    expect(mountApp(document)).toBeNull();
    expect(document.getElementById('app')).toBe(before);
  });
});
