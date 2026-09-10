export interface OfflineSearchStatus {
  phase: 'disabled' | 'downloading' | 'ready' | 'updating' | 'blocked' | 'error';
  activeVersion?: string | null;
  targetVersion?: string | null;
  base?: string;
  downloadedFiles?: number;
  totalFiles?: number;
  downloadedBytes?: number;
  totalBytes?: number;
  error?: string | null;
}

export interface OfflineStatus {
  requested: number;
  total: number;
  saved: Array<{ url: string; title: string }>;
  failed: number;
  downloading: boolean;
  search?: OfflineSearchStatus;
}

export function offlineSummary(state: OfflineStatus | null, enabled: boolean): string {
  if (!enabled) return '';
  if (!state) return 'Preparing offline storage…';
  if (!state.requested) return 'Automatic downloads are off. Previously visited pages may still be cached.';
  const articles = `${state.downloading ? 'Downloading' : 'Available offline'}: ${state.saved.length} of ${state.total} articles, with their retained images.`;
  const partial = state.failed ? ' Some article downloads are incomplete. Reconnect, lower the count, or free browser storage to retry.' : '';
  const search = state.search;
  if (!search) return articles + partial;
  if (search.phase === 'ready') return articles + partial + ' Full archive search is available offline; only saved articles include offline pages and media.';
  if (search.phase === 'downloading' || search.phase === 'updating') {
    return articles + partial + ` Downloading ${search.downloadedFiles || 0} of ${search.totalFiles || 0} search files.`
      + (search.activeVersion ? ' The previous search index remains available.' : ' Offline search will be ready when the download finishes.');
  }
  if (search.phase === 'error' || search.phase === 'blocked') {
    return articles + partial + ' Search download incomplete.'
      + (search.error ? ` ${search.error}` : '')
      + (search.activeVersion ? ' The previous search index remains available.' : ' Reconnect or free browser storage to retry.');
  }
  return articles + partial;
}
