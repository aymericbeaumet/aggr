import { describe, expect, it } from 'vitest';
import { fixture, render } from '../test/fixtures';

describe('embedded Svelte renderer', () => {
  it('renders complete pages without browser or Node globals', () => {
    for (const name of ['page-river', 'page-article', 'page-browse']) {
      const html = render({ page: fixture(name), body_html: '<p>Article text</p>' });
      expect(html).toMatch(/^<!doctype html>/);
      expect(html).toContain('<main');
      expect(html).toContain('id="aggr-page"');
      if (fixture(name).site.assets.app) expect(html).toContain('type="module"');
    }
  });
  it('escapes model and metadata text without changing the JSON value', () => {
    const page = fixture('page-river');
    page.head.title = '</title><script>bad()</script>';
    page.title = '</script><script>bad()</script>';
    const html = render({ page, body_html: '' });
    expect(html).not.toContain('<script>bad()');
    expect(html).toContain('\\u003c/script\\u003e');
    const json = html.match(/<script id="aggr-page" type="application\/json">(.*?)<\/script>/s)![1];
    expect(JSON.parse(json).title).toBe(page.title);
  });
  it('keeps renders independent when one runtime serves different pages', () => {
    const page = fixture('page-river');
    const first = render({ page, body_html: '' });
    render({ page: fixture('page-article'), body_html: '<p>Private article content</p>' });
    expect(render({ page, body_html: '' })).toBe(first);
  });
  it('publishes styles before preference bootstrap and emits the client entry', () => {
    const page = fixture('page-river');
    page.site.assets.app = 'assets/app/app-hash.js';
    page.site.assets.css = 'assets/app/app-hash.css';
    const html = render({ page, body_html: '' });
    expect(html.indexOf('app-hash.css')).toBeLessThan(html.indexOf('src="./assets/app/bootstrap.js"'));
    expect(html).toContain('<script type="module" src="./assets/app/app-hash.js">');
  });
  it('renders native media, hosted video, document and interactive fallbacks without JavaScript', () => {
    const page = fixture('page-article');
    if (page.page.view !== 'article') throw new Error('article fixture');
    const cases = [
      [{ kind: 'native_audio', url: 'assets/episode.mp3' }, '<audio'],
      [{ kind: 'native_video', url: 'assets/movie.mp4' }, '<video'],
      [{ kind: 'document', url: 'assets/paper.pdf' }, '<object'],
      [{ kind: 'interactive', url: 'https://publisher.test/demo' }, '<iframe'],
      [{ kind: 'video', provider: 'youtube', title: 'YouTube', embed_url: 'https://www.youtube.com/embed/id', requires_parent: false }, 'data-video-embed'],
    ] as const;
    for (const [media, marker] of cases) {
      page.page.data.media = media;
      expect(render({ page, body_html: '<p>Notes</p>' })).toContain(marker);
    }
  });

});
