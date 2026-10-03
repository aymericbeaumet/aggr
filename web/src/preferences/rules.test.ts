import { describe, expect, it } from 'vitest';
import { applyDataset, coerce, defaults, parseSchema, positiveInteger, readPreferences, valid } from './rules';

const schema = parseSchema(
  JSON.stringify({
    'date-format': { initial: 'relative', values: ['relative', 'iso', 'local', 'local-time'], attribute: 'dateFormat' },
    'feed-page-size': { initial: 50, values: [10, 25, 50] },
    'single-key-shortcuts': { initial: true, values: [true, false] },
    'scroll-amount': { initial: 10, min: 1, max: 100 },
  }),
);

describe('parseSchema', () => {
  it('accepts an object and nothing else', () => {
    expect(Object.keys(schema)).toHaveLength(4);
    expect(parseSchema(null)).toEqual({});
    expect(parseSchema('[]')).toEqual({});
    expect(parseSchema('{')).toEqual({});
  });
});

describe('valid', () => {
  it('checks choices and integer ranges', () => {
    expect(valid(schema['date-format'], 'iso')).toBe(true);
    expect(valid(schema['date-format'], 'ISO')).toBe(false);
    expect(valid(schema['feed-page-size'], 25)).toBe(true);
    expect(valid(schema['feed-page-size'], 30)).toBe(false);
    expect(valid(schema['scroll-amount'], 1)).toBe(true);
    expect(valid(schema['scroll-amount'], 100)).toBe(true);
    expect(valid(schema['scroll-amount'], 101)).toBe(false);
    expect(valid(schema['scroll-amount'], 2.5)).toBe(false);
    expect(valid(schema['scroll-amount'], '5')).toBe(false);
  });
});

describe('coerce', () => {
  it('restores the type the initial value declares', () => {
    expect(coerce(schema['single-key-shortcuts'], 'false')).toBe(false);
    expect(coerce(schema['single-key-shortcuts'], 'no')).toBeNull();
    expect(coerce(schema['scroll-amount'], '12')).toBe(12);
    expect(coerce(schema['scroll-amount'], '-1')).toBeNull();
    expect(coerce(schema['date-format'], 'local')).toBe('local');
    expect(coerce(schema['date-format'], null)).toBeNull();
  });
});

describe('readPreferences', () => {
  it('takes valid stored values and falls back to the initial ones', () => {
    const stored: Record<string, string> = { 'date-format': 'iso', 'feed-page-size': '30', 'scroll-amount': '20' };
    expect(readPreferences(schema, (key) => stored[key] ?? null)).toEqual({
      'date-format': 'iso',
      'feed-page-size': 50,
      'single-key-shortcuts': true,
      'scroll-amount': 20,
    });
  });

  it('survives storage that throws', () => {
    expect(
      readPreferences(schema, () => {
        throw new Error('private');
      })['date-format'],
    ).toBe('relative');
  });
});

describe('positiveInteger', () => {
  it('parses what it can', () => {
    expect(positiveInteger('3', 1)).toBe(3);
    expect(positiveInteger(0, 1)).toBe(1);
    expect(positiveInteger(undefined, 7)).toBe(7);
    expect(positiveInteger('2.9', 1)).toBe(2);
  });
});

describe('defaults', () => {
  it('is every setting at its initial value', () => {
    expect(defaults(schema)).toEqual({
      'date-format': 'relative',
      'feed-page-size': 50,
      'single-key-shortcuts': true,
      'scroll-amount': 10,
    });
  });
});

describe('applyDataset', () => {
  it('writes only the settings that drive CSS, as strings', () => {
    const root = { dataset: {} as Record<string, string> } as unknown as HTMLElement;
    applyDataset(schema, { 'date-format': 'iso', 'feed-page-size': 25, 'scroll-amount': 3 }, root);
    expect(root.dataset).toEqual({ dateFormat: 'iso' });
  });
});
