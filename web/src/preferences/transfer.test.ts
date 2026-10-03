import { describe, expect, it } from 'vitest';
import { parseSchema } from './rules';
import {
  decodeState,
  encodeState,
  envelope,
  exportText,
  fragmentState,
  parseTransfer,
  shareLink,
  TEXT_LIMIT,
  validate,
} from './transfer';

const schema = parseSchema(
  JSON.stringify({
    theme: { initial: 'system', values: ['system', 'light', 'dark'], attribute: 'theme' },
    'single-key-shortcuts': { initial: true, values: [true, false] },
    'scroll-amount': { initial: 10, min: 1, max: 100 },
  }),
);

describe('validate', () => {
  it('accepts a versioned envelope of known, valid settings', () => {
    expect(validate(schema, { version: 1, preferences: { theme: 'dark', 'scroll-amount': 20 } })).toEqual({
      theme: 'dark',
      'scroll-amount': 20,
    });
  });

  it('rejects everything that is not the envelope', () => {
    expect(() => validate(schema, null)).toThrow('Invalid preferences');
    expect(() => validate(schema, [])).toThrow('Invalid preferences');
    expect(() => validate(schema, { preferences: { theme: 'dark' } })).toThrow('Unsupported preferences version');
    expect(() => validate(schema, { version: 2, preferences: { theme: 'dark' } })).toThrow('Unsupported');
    expect(() => validate(schema, { version: 1, preferences: { theme: 'dark' }, history: [] })).toThrow('Unsupported');
    expect(() => validate(schema, { version: 1, preferences: {} })).toThrow('Empty preferences');
    expect(() => validate(schema, { version: 1, preferences: [] })).toThrow('Empty preferences');
    // A storage-key export names settings the schema does not know.
    expect(() => validate(schema, { version: 1, preferences: { 'aggr:theme': 'dark' } })).toThrow('Invalid preference: aggr:theme');
    expect(() => validate(schema, { version: 1, preferences: { theme: 'ultraviolet' } })).toThrow('Invalid preference: theme');
    expect(() => validate(schema, { version: 1, preferences: { toString: 'x' } })).toThrow('Invalid preference: toString');
  });
});

describe('parseTransfer', () => {
  it('bounds the text before parsing it', () => {
    expect(parseTransfer(schema, '{"version":1,"preferences":{"theme":"light"}}')).toEqual({ theme: 'light' });
    expect(() => parseTransfer(schema, ' '.repeat(TEXT_LIMIT + 1))).toThrow('too large');
    expect(() => parseTransfer(schema, '{')).toThrow();
  });
});

describe('links', () => {
  const values = { theme: 'dark', 'single-key-shortcuts': false, 'scroll-amount': 7 };

  it('round-trip the envelope through base64url', () => {
    const encoded = encodeState(values);
    expect(encoded).toMatch(/^[A-Za-z0-9_-]+$/);
    expect(JSON.parse(decodeState(encoded))).toEqual(envelope(values));
  });

  it('carry the payload in the fragment of the preferences page, never the query', () => {
    const url = new URL(shareLink('https://x.test/reader/', values));
    expect(url.pathname).toBe('/reader/preferences/');
    expect(url.search).toBe('');
    expect(fragmentState(url.hash)).toBe(encodeState(values));
    expect(fragmentState('')).toBeNull();
    expect(fragmentState('#other=1')).toBeNull();
  });

  it('refuse what is not base64url or too long to be a link', () => {
    expect(() => decodeState('not_valid!')).toThrow('Invalid preferences link');
    expect(() => decodeState('')).toThrow('Invalid preferences link');
    expect(() => decodeState('A'.repeat(22_001))).toThrow('Invalid preferences link');
  });
});

describe('exportText', () => {
  it('is the pretty envelope with a final newline', () => {
    const text = exportText({ theme: 'light' });
    expect(text.endsWith('}\n')).toBe(true);
    expect(JSON.parse(text)).toEqual({ version: 1, preferences: { theme: 'light' } });
  });
});
