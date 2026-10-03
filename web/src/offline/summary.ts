import type { OfflineStatus } from '../sw/messages';

/** What the offline page says about the downloads, in the words the reader is used to. */
export function offlineSummary(state: OfflineStatus | null, enabled: boolean): string {
  if (!enabled) return 'Offline storage is disabled for this site.';
  if (!state) return 'Preparing offline storage…';
  if (!state.requested) return 'Automatic downloads are off. Previously visited pages may still be cached.';
  let text = `${state.downloading ? 'Downloading' : 'Available offline'}: ${state.saved.length} of ${state.total} articles, with their retained images.`;
  if (state.failed) text += ' Some downloads are incomplete. Reconnect, lower the count, or free browser storage to retry.';
  const search = state.search;
  if (search?.phase === 'ready') text += ' Full archive search is available offline; only saved articles include pages and images.';
  else if (search?.phase === 'downloading' || search?.phase === 'updating') text += ` Downloading ${search.downloadedFiles} of ${search.totalFiles} search files.`;
  else if (search?.phase === 'blocked' || search?.phase === 'error') text += ' Search download incomplete. Reconnect or free browser storage to retry.';
  if (search?.activeVersion && search.phase !== 'ready') text += ' The previous search index remains available.';
  return text;
}
