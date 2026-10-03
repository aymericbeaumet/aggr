import { describe, expect, it } from 'vitest';
import { decodeSelection, encodeSelection, sharedSelection, words } from './selection-share';

describe('selection tokens', () => {
  it('round-trip a word range as base64url', () => {
    const token = encodeSelection(12, 40);
    expect(token).toMatch(/^[A-Za-z0-9_-]+$/);
    expect(decodeSelection(token)).toEqual([12, 40]);
    expect(decodeSelection(encodeSelection(0, 1))).toEqual([0, 1]);
  });

  it('reject anything that is not an increasing pair of safe integers', () => {
    expect(decodeSelection('')).toBeNull();
    expect(decodeSelection('!!')).toBeNull();
    expect(decodeSelection(btoa('5,5'))).toBeNull();
    expect(decodeSelection(btoa('9,3'))).toBeNull();
    expect(decodeSelection(btoa('a,b'))).toBeNull();
    expect(decodeSelection(btoa('1,2,3'))).toBeNull();
    expect(decodeSelection(btoa('1,99999999999999999999'))).toBeNull();
  });

  it('are read from the fragment, alone or among other parameters', () => {
    const token = encodeSelection(3, 7);
    expect(sharedSelection(`#selection=${token}`)).toEqual([3, 7]);
    expect(sharedSelection(`#aggr-state=x&selection=${token}`)).toEqual([3, 7]);
    expect(sharedSelection('#fn-1')).toBeNull();
    expect(sharedSelection('#selection=')).toBeNull();
    expect(sharedSelection('')).toBeNull();
  });
});

describe('words', () => {
  it('spans every run of non-space characters', () => {
    expect(words('Hello,  big\nworld ')).toEqual([
      { start: 0, end: 6 },
      { start: 8, end: 11 },
      { start: 12, end: 17 },
    ]);
    expect(words('   ')).toEqual([]);
  });
});
