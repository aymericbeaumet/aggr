import { describe, expect, it } from 'vitest';
import { decodeSelection, encodeSelection, selectionToken, wordRange, wordSpans } from './share-selection';

describe('shared selections', () => {
  it('addresses a selection by the words it covers', () => {
    const text = '  Alpha beta\n gamma  delta ';
    const words = wordSpans(text);
    expect(words.map(word => text.slice(word.start, word.end))).toEqual(['Alpha', 'beta', 'gamma', 'delta']);
    // A selection that starts and ends mid-word still shares whole words.
    expect(wordRange(words, 4, 11)).toEqual([0, 2]);
    expect(wordRange(words, 2, 27)).toEqual([0, 4]);
    // Whitespace between words touches nothing.
    expect(wordRange(words, 0, 2)).toBeNull();
    expect(wordRange(words, 12, 13)).toBeNull();
  });

  it('round-trips a word range through a URL-safe token', () => {
    for (const [start, end] of [[0, 1], [7, 9], [1234, 5678]] as const) {
      const token = encodeSelection(start, end);
      expect(token).toMatch(/^[A-Za-z0-9_-]+$/);
      expect(decodeSelection(token)).toEqual([start, end]);
    }
  });

  it('rejects tokens that do not describe a forward range', () => {
    for (const token of ['', 'not-base64!!', encodeSelection(5, 5), btoa('3,2'), btoa('a,b'), btoa('-1,4')]) {
      expect(decodeSelection(token)).toBeNull();
    }
  });

  it('reads the selection fragment without disturbing ordinary anchors', () => {
    expect(selectionToken('#selection=MCwx')).toBe('MCwx');
    expect(selectionToken('#a-heading')).toBeNull();
    expect(selectionToken('')).toBeNull();
  });
});
