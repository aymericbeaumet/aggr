import { describe, expect, it } from 'vitest';
import { bootstrap, watchDates } from './bootstrap';

const rules = {
  theme: { initial: 'system', values: ['system', 'light', 'dark'], attribute: 'theme' },
  density: { initial: 'compact', values: ['compact', 'comfortable'], attribute: 'density' },
  'date-format': { initial: 'relative', values: ['relative', 'iso', 'local', 'local-time'], attribute: 'dateFormat' },
};
const now = () => Date.parse('2026-09-03T12:00:00Z');

function load(body: string): void {
  document.head.innerHTML = `<script id="aggr-preferences" type="application/json">${JSON.stringify(rules)}</script>`;
  document.body.innerHTML = body;
  for (const key of ['theme', 'density', 'dateFormat']) delete document.documentElement.dataset[key];
}

const times =
  '<span class="published-date" data-date-tooltip title="Published: 2026-09-01T12:00:00Z"><time datetime="2026-09-01T12:00:00Z">2026-09-01</time></span>' +
  '<time datetime="PT5M">5 min read</time>' +
  '<time datetime="2026-09-02T12:00:00Z" title="2026-09-02T12:00:00Z">2026-09-02</time>';

describe('bootstrap', () => {
  it('puts stored, valid values on <html> and falls back for the rest', () => {
    load(times);
    const stored: Record<string, string> = { 'aggr:theme': 'dark', 'aggr:density': 'invalid' };
    bootstrap(document, (key) => stored[key] ?? null, now);
    expect(document.documentElement.dataset).toMatchObject({ theme: 'dark', density: 'compact', dateFormat: 'relative' });
    const [published, duration, plain] = document.querySelectorAll('time');
    expect(published.textContent).toBe('2d ago');
    // The accessible name the app will render: the text and the label's exact tooltip.
    expect(published.getAttribute('aria-label')).toBe('2d ago; Published: 2026-09-01T12:00:00Z');
    expect(published.parentElement?.getAttribute('title')).toBe('Published: 2026-09-01T12:00:00Z');
    expect(duration.textContent).toBe('5 min read');
    expect(plain.textContent).toBe('1d ago');
    expect(plain.getAttribute('aria-label')).toBe('1d ago; 2026-09-02T12:00:00Z');
    expect(window.AggrBootstrap).toBeUndefined();
  });

  it('formats dates in the stored style', () => {
    load(times);
    bootstrap(document, (key) => (key === 'aggr:date-format' ? 'iso' : null), now);
    expect(document.documentElement.dataset.dateFormat).toBe('iso');
    expect(document.querySelector('time')?.textContent).toBe('2026-09-01');
  });

  it('survives storage that throws', () => {
    load(times);
    bootstrap(
      document,
      () => {
        throw new Error('private');
      },
      now,
    );
    expect(document.documentElement.dataset.theme).toBe('system');
    expect(document.querySelector('time')?.textContent).toBe('2d ago');
  });

  it('rewrites dates as they stream in, until the document is parsed', async () => {
    const doc = document.implementation.createHTMLDocument('');
    Object.defineProperty(doc, 'readyState', { value: 'loading', configurable: true });
    const time = (stamp: string) => {
      const element = doc.createElement('time');
      element.setAttribute('datetime', stamp);
      element.textContent = stamp;
      return element;
    };
    const first = time('2026-09-01T12:00:00Z');
    doc.body.append(first);
    watchDates(doc, 'iso', now);
    expect(first.textContent).toBe('2026-09-01');
    const second = time('2026-09-02T12:00:00Z');
    doc.body.append(second);
    const later = doc.createElement('time');
    later.setAttribute('datetime', '2026-08-30T12:00:00Z');
    doc.body.append(later);
    later.textContent = '2026-08-30T12:00:00Z';
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(second.textContent).toBe('2026-09-02');
    expect(later.textContent).toBe('2026-08-30');
    doc.dispatchEvent(new Event('DOMContentLoaded'));
    const afterwards = time('2026-08-29T12:00:00Z');
    doc.body.append(afterwards);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(afterwards.textContent).toBe('2026-08-29T12:00:00Z');
  });
});

declare global {
  interface Window {
    AggrBootstrap?: unknown;
  }
}
