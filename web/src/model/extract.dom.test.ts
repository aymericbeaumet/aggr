import { describe, expect, it } from 'vitest';
import { fixture, render } from '../../test/fixtures';
import { extract, parseDocument } from './extract';

describe('page payload', () => {
  it('extracts the typed model and only the opaque article body', () => {
    const page = fixture('page-article');
    const doc = parseDocument(render({ page, body_html: '<p>Body</p>' }));
    const body = doc.querySelector('[data-article-body]');
    const payload = extract(doc);
    expect(payload?.model).toEqual(page);
    expect(payload?.bodyHtml).toBe('<p>Body</p>');
    expect(doc.querySelector('[data-article-body]')).toBe(body);
  });
  it('rejects malformed or absent page models', () => {
    expect(extract(parseDocument('<p>Not a reader</p>'))).toBeNull();
    expect(extract(parseDocument('<script id="aggr-page">{</script>'))).toBeNull();
  });
});
