import { describe, expect, it } from 'vitest';
import { parse } from './parse';

describe('parse', () => {
  it('returns the decoded JSON value', () => {
    expect(parse('{"items":[1,2]}')).toEqual({ items: [1, 2] });
  });

  it('rejects malformed input', () => {
    expect(() => parse('{')).toThrow(SyntaxError);
  });
});
