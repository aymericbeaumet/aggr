import { beforeEach, describe, expect, it } from "../../test/browser";
import { load } from '../../test/fixtures';
import { mountApp } from './boot';
import { page } from '../state/page.svelte';

describe('hydration', () => {
  beforeEach(() => { sessionStorage.clear(); });
  it('preserves the server-rendered app, main and feed rows', () => {
    load('page-river');
    const root = document.getElementById('app');
    const main = document.getElementById('content');
    const row = document.querySelector('.row');
    const shown = mountApp(document);
    expect(shown?.model.kind).toBe('river');
    expect(document.getElementById('app')).toBe(root);
    expect(document.getElementById('content')).toBe(main);
    expect(document.querySelector('.row')).toBe(row);
    expect(page.root).toBe(new URL('./', document.baseURI).href);
  });
  it('preserves opaque article prose and does not duplicate it in the model', () => {
    load('page-article', '<p id="opaque">A preserved paragraph.</p>');
    const paragraph = document.getElementById('opaque');
    expect(paragraph).not.toBeNull();
    expect(document.getElementById('aggr-page')?.textContent).not.toContain('preserved paragraph');
    mountApp(document);
    expect(document.getElementById('opaque')).toBe(paragraph);
    expect(document.querySelectorAll('[data-article-body]')).toHaveLength(1);
  });
  it('leaves a document without a model alone', () => {
    document.head.innerHTML = ''; document.body.innerHTML = '<main>Static</main>';
    expect(mountApp(document)).toBeNull();
    expect(document.querySelector('main')?.textContent).toBe('Static');
  });
});
