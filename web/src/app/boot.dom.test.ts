/// <reference types="node" />
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { beforeEach, describe, expect, it } from 'vitest';
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

describe('mountApp', () => {
  beforeEach(() => {
    sessionStorage.clear();
  });

  it('preserves Rust-authored page, header and row nodes through enhancement', () => {
    load('page-river');
    const app = document.getElementById('app');
    const header = document.querySelector('header.top');
    const row = document.querySelector('.row');
    const note = document.createElement('strong');
    note.textContent = 'Template-only presentation';
    row?.append(note);
    mountApp(document);
    expect(document.getElementById('app')).toBe(app);
    expect(document.querySelector('header.top')).toBe(header);
    expect(document.querySelector('.row')).toBe(row);
    expect(note.isConnected).toBe(true);
  });

  it('enhances the static page and selects its first row without replacing the app', () => {
    load('page-river');
    const before = document.getElementById('app');
    const shown = mountApp(document);
    expect(shown?.model.kind).toBe('river');
    const app = document.getElementById('app');
    expect(app).toBe(before);
    expect(app?.parentNode).toBe(document.body);
    expect(page.root).toBe(new URL('./', document.baseURI).href);
    // The cursor came back on the first row, as the vanilla reader restored it.
    const selected = document.querySelectorAll('.row.is-selected');
    expect(selected).toHaveLength(1);
    expect(selected[0]).toBe(document.querySelector('.rows .row'));
  });

  it('preserves the scoped list and its error', () => {
    load('page-source-error');
    const heading = document.querySelector('.listhead');
    const text = heading?.textContent;
    mountApp(document);
    expect(document.querySelector('.listhead')).toBe(heading);
    expect(heading?.textContent).toBe(text);
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
  });

  it('leaves a page without a model alone', () => {
    load('page-river');
    document.getElementById('aggr-page')?.remove();
    const before = document.getElementById('app');
    expect(mountApp(document)).toBeNull();
    expect(document.getElementById('app')).toBe(before);
  });
});
