import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'svelte/server';
import Metadata from './Metadata.svelte';
import SearchDate from './SearchDate.svelte';
import { createDates } from '../dates';
import { dateLabel, dateTooltip, displayData } from './display';
import type { ItemMetadata } from './types';

function result(metadata: ItemMetadata) {
  const opaque = Buffer.from(JSON.stringify(metadata), 'utf8').toString('hex');
  return displayData({ url: '/article/', meta: { aggr_display: opaque } });
}
function html(metadata: ItemMetadata) {
  vi.stubGlobal('window', {});
  return render(Metadata, { props: {
    metadata: result(metadata), base: 'https://reader.test/aggr/', dateFormat: 'iso',
    original: 'https://publisher.test/article',
  } }).body;
}
afterEach(() => vi.unstubAllGlobals());

describe('shared reader metadata', () => {
  it('renders opaque display fields with zero counts, discussion scores, and escaped labels', () => {
    const rendered = html({
      source_slug: 'publisher', source_display: 'publisher<&>', source_title: 'Publisher "quoted"',
      date: '2026-09-09T10:00:00Z', updated: '2026-09-09T11:00:00Z',
      is_aggregated: true, feed_display: 'feed.test/news',
      category: { slug: 'rust', name: 'rust & friends' },
      word_count: 1, reading_minutes: 1, points: 0,
      consumption: { action: 'read', minutes: 1, words: 1 },
      comments: { url: 'https://publisher.test/comments?a=1&b=2', count: 0 },
      discussions: [{ name: 'discussion<one>', url: 'https://forum.test/item', score: 12 }],
    });
    expect(rendered).toMatch(/publisher&lt;&amp;(?:>|&gt;)/);
    expect(rendered).toContain('Publisher &quot;quoted&quot;');
    expect(rendered.replace(/<!--.*?-->/g, '')).toContain('</span> <em>via feed.test/news');
    expect(rendered).toContain('rust &amp; friends');
    expect(rendered).toContain('title="1 word"');
    expect(rendered).toContain('0 points');
    expect(rendered).toContain('0 comments');
    expect(rendered).toContain('matching discussion found, score 12');
    expect(rendered).toContain('a=1&amp;b=2');
    expect(rendered).not.toContain(' · ');
    expect(rendered).toContain('data-date-updated="2026-09-09T11:00:00Z"');
  });

  it('omits absent counts, unchanged update dates and optional fields', () => {
    const rendered = html({ date: '2026-09-09', updated: '2026-09-09',
      comments: { url: 'https://forum.test/item' },
      discussions: [{ name: 'forum', url: 'https://forum.test/item' }],
    });
    expect(rendered).toContain('>comments</a>');
    expect(rendered).not.toContain('points');
    expect(rendered).not.toContain('score');
    expect(rendered).not.toContain('reading-stats');
    expect(rendered).not.toContain('data-date-updated');
    expect(rendered).not.toContain('undefined');
  });

  it('uses media duration instead of transcript reading time and keeps unknown duration honest', () => {
    for (const action of ['listen', 'watch'] as const) {
      const rendered = html({ word_count: 1_000, reading_minutes: 5, consumption: { action, minutes: 61, seconds: 3601 } });
      expect(rendered).toContain(`data-consumption="${action}"`);
      expect(rendered).toContain('data-duration-seconds="3601"');
      expect(rendered.replace(/<!--.*?-->/g, '')).toContain(`datetime="PT3601S">61 min ${action}</time>`);
      expect(rendered).not.toContain('min read');
      expect(rendered).not.toContain('words');
      const unknown = html({ word_count: 1_000, reading_minutes: 5, consumption: { action } });
      expect(unknown).toContain(action === 'listen' ? 'Listen' : 'Watch');
      expect(unknown).not.toContain('min ');
      expect(unknown).not.toContain('data-duration-seconds');
    }
  });

  it('keeps unsafe links inert in client metadata and handles invalid transport', () => {
    expect(html({ comments: { url: 'javascript:alert(1)' } })).not.toContain('href="javascript:');
    expect(displayData({ url: '/article/', meta: { aggr_display: 'not hex' } })).toEqual({});
    expect(displayData({ url: '/article/', meta: { aggr_display: '7b' } })).toEqual({});
    expect(displayData({ url: '/article/', meta: { aggr_display: '6e756c6c' } })).toEqual({});
  });
});

describe('Svelte-owned search dates', () => {
  it('renders updated clock and preference values without another DOM formatter', () => {
    const published = '2026-09-09T10:00:00Z';
    const initial = Date.parse(published) + 60_000;
    const view = (now: number, dateFormat = 'relative') => render(SearchDate, { props: { published, now, dateFormat } }).body;
    expect(view(initial)).toContain('>1m ago</time>');
    expect(view(initial + 60_000)).toContain('>2m ago</time>');
    expect(view(initial + 60_000, 'iso')).toContain('>2026-09-09</time>');
    expect(view(initial)).toContain('aria-label="1m ago; Published: 2026-09-09T10:00:00Z"');
    expect(dateLabel(published, 'local')).toBe(new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' }).format(Date.parse(published)));
    expect(dateLabel(published, 'local-time')).toBe(new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(Date.parse(published)));
  });

  it('localizes tooltips only on demand and omits invalid or unchanged updates', () => {
    const published = '2026-09-09T10:00:00Z';
    const updated = '2026-09-09T12:00:00Z';
    expect(dateTooltip(published, updated)).toBe(`Published: ${published}\nUpdated: ${updated}`);
    expect(dateTooltip(published, published)).toBe(`Published: ${published}`);
    expect(dateTooltip(published, 'invalid')).toBe(`Published: ${published}`);
    const localized = new Intl.DateTimeFormat(undefined, { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit' });
    expect(dateTooltip(published, updated, true)).toBe(`Published: ${localized.format(Date.parse(published))}\nUpdated: ${localized.format(Date.parse(updated))}`);
  });

  it('keeps the static date formatter outside search-owned nodes', () => {
    vi.stubGlobal('document', new EventTarget());
    const time = { closest: () => ({}), getAttribute: vi.fn(() => { throw new Error('Search dates belong to Svelte'); }) };
    const dates = createDates({ format: () => 'relative', afterFormat: vi.fn() });
    dates.formatTimes({ querySelectorAll: () => [time] } as unknown as ParentNode);
    expect(time.getAttribute).not.toHaveBeenCalled();
    dates.dispose();
  });
});
