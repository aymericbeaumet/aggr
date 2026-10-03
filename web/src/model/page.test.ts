import { describe, expect, it } from 'vitest';
import { isClientPage, readModel } from './page';

const model = {
  base: './',
  path: '',
  kind: 'river',
  title: 'Reader',
  site: { title: 'Reader', language: 'en', pwa: false, excerpts: true, discussions: [], entries: [], config_url: null, assets: { app: null, css: null, imports: [] } },
  build: { version: '1.0.0', app: 'a', content: 'c' },
  page: { view: 'static' },
};

describe('isClientPage', () => {
  it('accepts the embedded model shape', () => {
    expect(isClientPage(model)).toBe(true);
  });

  it('rejects anything else', () => {
    expect(isClientPage(null)).toBe(false);
    expect(isClientPage([])).toBe(false);
    expect(isClientPage({ ...model, kind: 1 })).toBe(false);
    expect(isClientPage({ ...model, build: { version: '1' } })).toBe(false);
    expect(isClientPage({ ...model, page: { view: 'unknown' } })).toBe(false);
    expect(isClientPage({ ...model, site: { ...model.site, entries: 'x' } })).toBe(false);
  });
});

describe('readModel', () => {
  it('decodes the script text and leaves the page static otherwise', () => {
    expect(readModel(JSON.stringify(model))).toEqual(model);
    expect(readModel('{')).toBeNull();
    expect(readModel('{"view":"list"}')).toBeNull();
    expect(readModel('')).toBeNull();
    expect(readModel(null)).toBeNull();
  });
});
