import type { Completion } from './completion';
import type { SearchPage } from './types';

export interface ViewState {
  query: string; cursor: number; active: boolean; ready: boolean; busy: boolean;
  error: string; offline: string; suggestions: Completion[]; open: boolean;
  page: SearchPage; now?: number; selectedURL: string; dateFormat: unknown; base: string;
  isOffline: boolean; hasOfflineStatus: boolean; savedURLs: Set<string>; cachedPreviews: Set<string>;
}
