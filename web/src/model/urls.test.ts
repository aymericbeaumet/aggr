import { describe, expect, it } from 'vitest';
import { facetPage, facetUrl, isSiteReference, sitePath, urlFor } from './urls';

describe('sitePath', () => {
  it('strips the root and the current-directory prefix', () => {
    expect(sitePath('items/blog/plain/')).toBe('items/blog/plain/');
    expect(sitePath('./')).toBe('');
    expect(sitePath('./page/2/')).toBe('page/2/');
    expect(sitePath('/sources/blog/')).toBe('sources/blog/');
  });
});

describe('urlFor', () => {
  it('joins site paths onto the page base', () => {
    expect(urlFor('./', '')).toBe('./');
    expect(urlFor('../../', './')).toBe('../../');
    expect(urlFor('../../../', 'items/blog/plain/')).toBe('../../../items/blog/plain/');
    expect(urlFor('./', 'browse/')).toBe('./browse/');
  });

  it('leaves absolute URLs alone', () => {
    expect(urlFor('../', 'https://blog.example/')).toBe('https://blog.example/');
    expect(urlFor('../', 'mailto:me@example.com')).toBe('mailto:me@example.com');
    expect(isSiteReference('data:image/png;base64,AAAA')).toBe(false);
    // A scheme is letters, digits, `+`, `-` and `.` only: a colon after a slash is not one.
    expect(isSiteReference('items/blog/x:y/')).toBe(true);
    expect(isSiteReference('items/blog/plain/')).toBe(true);
  });
});

describe('facetPage', () => {
  it('points at the collection page of the facet', () => {
    expect(facetPage('./', 'blog.example', 'source')).toBe('./sources/blog.example/');
    expect(facetPage('../', 'engineering', 'category')).toBe('../categories/engineering/');
    expect(facetPage('../../', 'rust', 'tag')).toBe('../../tags/rust/');
  });
});

describe('facetUrl', () => {
  it('encodes the query like form_urlencoded', () => {
    expect(facetUrl('./', 'engineering', 'category')).toBe('./?q=category%3A%22engineering%22');
    expect(facetUrl('../../../', 'rust', 'tag')).toBe('../../../?q=tag%3A%22rust%22');
  });

  it('quotes spaces and quotes inside the value', () => {
    expect(facetUrl('./', 'Web Dev', 'tag')).toBe('./?q=tag%3A%22Web+Dev%22');
    expect(facetUrl('./', 'say "hi"', 'tag')).toBe('./?q=tag%3A%22say+%5C%22hi%5C%22%22');
  });
});
