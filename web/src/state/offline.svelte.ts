import type { OfflineStatus, SavedArticle } from '../sw/messages';
import { search } from './search.svelte';

/**
 * What the service worker has committed for use without a network, as it last reported it
 * (`AGGR_OFFLINE_STATUS`, relayed by `offline/client.svelte.ts`). Search reads which index
 * version is saved, so that offline it selects that catalogue and runtime rather than the live
 * one; the offline page lists the saved articles.
 */
class Offline {
  /** The worker's last report; null until it has reported at all. */
  status = $state.raw<OfflineStatus | null>(null);
  /** Whether the site was built with a worker; false turns every report into "disabled". */
  enabled = $state(true);

  /** Whether the worker has reported its state at all. */
  get ready(): boolean {
    return this.status !== null;
  }

  /** The complete search index saved offline, when one is; null otherwise. */
  get search(): { activeVersion: string; base: string } | null {
    const saved = this.status?.search;
    return saved?.activeVersion && saved.base ? { activeVersion: saved.activeVersion, base: saved.base } : null;
  }

  /** Articles whose page and every retained rendition are stored. */
  get saved(): SavedArticle[] {
    return this.status?.saved ?? [];
  }

  /** The worker reported: the state follows, and a newly committed index reaches search. */
  receive(status: OfflineStatus): void {
    const before = this.search?.activeVersion ?? null;
    this.status = status;
    if ((this.search?.activeVersion ?? null) !== before) search.driver?.refresh();
  }
}

export const offline = new Offline();
