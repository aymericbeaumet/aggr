import { describe, expect, it } from 'vitest';
import type { OfflineStatus, SearchStatus } from '../sw/messages';
import { offlineSummary } from './summary';

const search = (phase: SearchStatus['phase'], activeVersion: string | null = null): SearchStatus => ({
  phase,
  activeVersion,
  targetVersion: 'v2',
  base: 'offline/',
  downloadedFiles: 3,
  totalFiles: 9,
  downloadedBytes: 0,
  totalBytes: 0,
  error: null,
});

const status = (fields: Partial<OfflineStatus> = {}): OfflineStatus => ({
  type: 'AGGR_OFFLINE_STATUS',
  requested: 30,
  total: 12,
  saved: Array.from({ length: 5 }, (_, index) => ({ url: `items/${index}/`, title: `Item ${index}` })),
  failed: 0,
  downloading: false,
  ...fields,
});

describe('offlineSummary', () => {
  it('says when storage is off, unknown, or not asked for', () => {
    expect(offlineSummary(null, false)).toBe('Offline storage is disabled for this site.');
    expect(offlineSummary(null, true)).toBe('Preparing offline storage…');
    expect(offlineSummary(status({ requested: 0 }), true)).toBe('Automatic downloads are off. Previously visited pages may still be cached.');
  });

  it('counts the articles and reports incomplete downloads', () => {
    expect(offlineSummary(status(), true)).toBe('Available offline: 5 of 12 articles, with their retained images.');
    expect(offlineSummary(status({ downloading: true, failed: 2 }), true)).toBe(
      'Downloading: 5 of 12 articles, with their retained images. Some downloads are incomplete. Reconnect, lower the count, or free browser storage to retry.',
    );
  });

  it('describes the search index alongside', () => {
    expect(offlineSummary(status({ search: search('ready', 'v2') }), true)).toContain('Full archive search is available offline');
    expect(offlineSummary(status({ search: search('downloading') }), true)).toContain('Downloading 3 of 9 search files.');
    expect(offlineSummary(status({ search: search('blocked', 'v1') }), true)).toBe(
      'Available offline: 5 of 12 articles, with their retained images. Search download incomplete. Reconnect or free browser storage to retry. The previous search index remains available.',
    );
  });
});
