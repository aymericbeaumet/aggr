import { afterEach, describe, expect, it, vi } from 'vitest';
import { offline } from '../state/offline.svelte';
import { search, type SearchDriver } from '../state/search.svelte';
import type { OfflineStatus } from '../sw/messages';
import { receive } from './client.svelte';

const status = (activeVersion: string | null): OfflineStatus => ({
  type: 'AGGR_OFFLINE_STATUS',
  requested: 2,
  total: 2,
  saved: [{ url: 'items/a/', title: 'A' }],
  failed: 0,
  downloading: false,
  search: {
    phase: activeVersion ? 'ready' : 'downloading',
    activeVersion,
    targetVersion: 'v2',
    base: 'offline/',
    downloadedFiles: 1,
    totalFiles: 1,
    downloadedBytes: 1,
    totalBytes: 1,
    error: null,
  },
});

describe('the worker relay', () => {
  afterEach(() => {
    offline.status = null;
    search.driver = null;
  });

  it('takes a status from the controlling worker only', () => {
    const controller = {} as ServiceWorker;
    const other = {} as ServiceWorker;
    expect(receive({ data: status('v1'), source: other }, controller)).toBe(false);
    expect(receive({ data: status('v1'), source: controller }, null)).toBe(false);
    expect(receive({ data: { type: 'AGGR_BUILD', app_version: 'a', content_version: 'c' }, source: controller }, controller)).toBe(false);
    expect(receive({ data: 'text', source: controller }, controller)).toBe(false);
    expect(offline.ready).toBe(false);
    expect(receive({ data: status('v1'), source: controller }, controller)).toBe(true);
    expect(offline.ready).toBe(true);
    expect(offline.saved).toEqual([{ url: 'items/a/', title: 'A' }]);
    expect(offline.search).toEqual({ activeVersion: 'v1', base: 'offline/' });
  });

  it('tells search when a committed index changes, and only then', () => {
    const refresh = vi.fn();
    search.driver = { refresh } as unknown as SearchDriver;
    offline.receive(status(null));
    expect(offline.search).toBeNull();
    expect(refresh).not.toHaveBeenCalled();
    offline.receive(status('v1'));
    expect(refresh).toHaveBeenCalledTimes(1);
    offline.receive({ ...status('v1'), downloading: true });
    expect(refresh).toHaveBeenCalledTimes(1);
    offline.receive(status('v2'));
    expect(refresh).toHaveBeenCalledTimes(2);
  });
});
