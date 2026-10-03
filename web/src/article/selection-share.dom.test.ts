import { afterEach, describe, expect, it } from 'vitest';
import { indexText, locate, wordRange } from './selection-share';

function body(html: string): HTMLElement {
  const element = document.createElement('div');
  element.className = 'body';
  element.innerHTML = html;
  document.body.append(element);
  return element;
}

afterEach(() => {
  document.body.innerHTML = '';
});

describe('indexText', () => {
  it('lays the text nodes end to end and counts words across them', () => {
    const index = indexText(body('<p>Hello <em>big</em> world.</p><p>Again</p>'));
    expect(index.text).toBe('Hello big world.Again');
    expect(index.nodes.map((entry) => entry.start)).toEqual([0, 6, 9, 16]);
    expect(index.words).toEqual([
      { start: 0, end: 5 },
      { start: 6, end: 9 },
      { start: 10, end: 21 },
    ]);
  });
});

describe('wordRange', () => {
  const index = indexText(body('<p>one two three four</p>'));

  it('covers every word the characters touch, half-open', () => {
    expect(wordRange(index, 0, 3)).toEqual([0, 1]);
    expect(wordRange(index, 2, 9)).toEqual([0, 3]);
    expect(wordRange(index, 4, 7)).toEqual([1, 2]);
    expect(wordRange(index, 14, 18)).toEqual([3, 4]);
  });

  it('has nothing for spaces alone or an empty or reversed span', () => {
    expect(wordRange(index, 3, 4)).toBeNull();
    expect(wordRange(index, 5, 5)).toBeNull();
    expect(wordRange(index, 9, 2)).toBeNull();
  });
});

describe('locate', () => {
  it('finds the text nodes and offsets a word range starts and ends in', () => {
    const root = body('<p>Hello <em>big</em> world.</p>');
    const index = indexText(root);
    const em = root.querySelector('em')?.firstChild as Text;
    const last = root.querySelector('p')?.lastChild as Text;
    expect(locate(index, [1, 2])).toEqual({ start: [em, 0], end: [em, 3] });
    expect(locate(index, [1, 3])).toEqual({ start: [em, 0], end: [last, 7] });
    expect(locate(index, [0, 1])).toEqual({ start: [root.querySelector('p')?.firstChild as Text, 0], end: [root.querySelector('p')?.firstChild as Text, 5] });
  });

  it('gives up on a range past the article', () => {
    const index = indexText(body('<p>one two</p>'));
    expect(locate(index, [1, 5])).toBeNull();
    expect(locate(index, [7, 8])).toBeNull();
  });

  it('puts a restored range back under selection', () => {
    const root = body('<p>Hello <em>big</em> world.</p>');
    const found = locate(indexText(root), [1, 3]);
    const range = document.createRange();
    if (!found) throw new Error('not located');
    range.setStart(...found.start);
    range.setEnd(...found.end);
    expect(range.toString()).toBe('big world.');
  });
});
