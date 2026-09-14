import { describe, expect, it } from 'vitest';
import { offlineSummary } from './offline';

describe('offline availability', () => {
  const articles = { requested: 2, total: 2, saved: [{ url: 'items/one/', title: 'One' }], failed: 0, downloading: false };
  it('keeps saved articles distinct from searchable articles', () => {
    expect(offlineSummary({ ...articles, search: { phase: 'ready', activeVersion: 'a' } }, true))
      .toContain('1 of 2 articles');
    expect(offlineSummary({ ...articles, search: { phase: 'ready', activeVersion: 'a' } }, true))
      .toContain('Full archive search is available offline');
  });
  it('does not report an incomplete index as ready', () => {
    expect(offlineSummary({ ...articles, search: { phase: 'downloading', downloadedFiles: 2, totalFiles: 8 } }, true))
      .toContain('2 of 8 search files');
    expect(offlineSummary({ ...articles, search: { phase: 'error', activeVersion: 'old', error: 'Storage is full.' } }, true))
      .toContain('previous search index remains available');
  });
  it('hides unavailable storage while reporting disabled automatic downloads', () => {
    expect(offlineSummary(null, false)).toBe('');
    expect(offlineSummary(articles, false)).toBe('');
    expect(offlineSummary({ ...articles, requested: 0 }, true)).toContain('Automatic downloads are off');
  });
});
