import { describe, expect, it } from 'vitest';
import { row } from './fixture';
import { rowHTML } from './row';

describe('Rust search row fragments', () => {
  it('retains template-specific markup while rebasing site URLs on a nested article route', () => {
    const source = { ...row, html: '<li class="row" data-template-marker="rust"><span class="rank">1.</span><a data-row-open href="items/blog/story/">Story</a><a class="source" href="sources/blog/">Blog</a><a class="query" href="?q=category%3Arust">Rust</a><a class="original" href="https://publisher.test/story">Original</a><img src="assets/preview.jpg"><img class="placeholder" src="data:image/png;base64,AA=="></li>' };
    const target = document.createElement('template');
    target.innerHTML = rowHTML(source, 'https://reader.test/collection/');
    const item = target.content.querySelector('li');
    expect(item?.dataset.templateMarker).toBe('rust');
    expect(item?.querySelector('[data-row-open]')?.getAttribute('href')).toBe('https://reader.test/collection/items/blog/story/');
    expect(item?.querySelector('.source')?.getAttribute('href')).toBe('https://reader.test/collection/sources/blog/');
    expect(item?.querySelector('.query')?.getAttribute('href')).toBe('https://reader.test/collection/?q=category%3Arust');
    expect(item?.querySelector('.original')?.getAttribute('href')).toBe('https://publisher.test/story');
    expect(item?.querySelector('img')?.getAttribute('src')).toBe('https://reader.test/collection/assets/preview.jpg');
    expect(item?.querySelector('.placeholder')?.getAttribute('src')).toBe('data:image/png;base64,AA==');
  });

  it('rejects malformed fragments instead of introducing another renderer', () => {
    expect(rowHTML({ ...row, html: '<div>Wrong element</div>' }, 'https://reader.test/')).toBe('');
    expect(rowHTML({ ...row, html: '<li class="row"></li><li class="row"></li>' }, 'https://reader.test/')).toBe('');
  });
});
