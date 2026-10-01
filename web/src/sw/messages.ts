// The message protocol between the reader pages and the service worker. Nothing here depends on
// a platform lib, so the page side imports the same types.

export type SearchPhase = 'disabled' | 'downloading' | 'updating' | 'ready' | 'blocked' | 'error';

export type SearchError = 'network' | 'integrity' | 'evicted' | 'quota';

export interface SearchStatus {
  phase: SearchPhase;
  /** The complete index answering offline queries, if any. */
  activeVersion: string | null;
  /** The index of the current build, being downloaded or already active. */
  targetVersion: string | null;
  base: string | null;
  downloadedFiles: number;
  totalFiles: number;
  downloadedBytes: number;
  totalBytes: number;
  error: SearchError | null;
}

export interface SavedArticle {
  url: string;
  title: string;
}

/** Broadcast to every window whenever the offline state changes, and answered on request. */
export interface OfflineStatus {
  type: 'AGGR_OFFLINE_STATUS';
  /** The configured download limit. */
  requested: number;
  /** How many catalogue articles that limit covers. */
  total: number;
  /** Articles whose page and every rendition are stored. */
  saved: SavedArticle[];
  failed: number;
  downloading: boolean;
  search?: SearchStatus;
  /** A newer configuration superseded this download before it finished. */
  cancelled?: boolean;
}

/** The versions the running worker was built from, answering `AGGR_GET_BUILD`. */
export interface BuildMessage {
  type: 'AGGR_BUILD';
  app_version: string;
  content_version: string;
}

export type WorkerMessage = OfflineStatus | BuildMessage;

/** A page that loaded while the worker was activating asks to be controlled. */
export interface ClaimMessage {
  type: 'claim';
}

/** Save the first `count` catalogue articles, and the search index when `count` is positive. */
export interface OfflineConfigMessage {
  type: 'AGGR_OFFLINE_CONFIG';
  count: number;
}

export interface OfflineStatusRequest {
  type: 'AGGR_OFFLINE_GET_STATUS';
}

export interface BuildRequest {
  type: 'AGGR_GET_BUILD';
}

export type PageMessage = ClaimMessage | OfflineConfigMessage | OfflineStatusRequest | BuildRequest;

/** The most articles a configuration may select; the catalogue never lists more. */
export const OFFLINE_COUNT_LIMIT = 1000;

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function validOfflineCount(value: unknown): value is number {
  return typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= OFFLINE_COUNT_LIMIT;
}

/** The page messages the worker acts on; anything else, an out-of-range count included, is ignored. */
export function parsePageMessage(data: unknown): PageMessage | null {
  if (!isRecord(data)) return null;
  switch (data.type) {
    case 'claim':
      return { type: 'claim' };
    case 'AGGR_OFFLINE_GET_STATUS':
      return { type: 'AGGR_OFFLINE_GET_STATUS' };
    case 'AGGR_GET_BUILD':
      return { type: 'AGGR_GET_BUILD' };
    case 'AGGR_OFFLINE_CONFIG':
      return validOfflineCount(data.count) ? { type: 'AGGR_OFFLINE_CONFIG', count: data.count } : null;
    default:
      return null;
  }
}

export function isOfflineStatus(data: unknown): data is OfflineStatus {
  return isRecord(data) && data.type === 'AGGR_OFFLINE_STATUS' && Array.isArray(data.saved);
}

export function isBuildMessage(data: unknown): data is BuildMessage {
  return (
    isRecord(data) &&
    data.type === 'AGGR_BUILD' &&
    typeof data.app_version === 'string' &&
    typeof data.content_version === 'string'
  );
}
