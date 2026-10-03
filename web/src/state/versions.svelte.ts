import { page } from './page.svelte';

/** What `updates.json` says about the deployment: the shape the poller accepts. */
export type Build = { app_version: string; content_version: string; entries?: string[] };

/**
 * The versions in play: the page on screen carries the app and content versions it was built
 * from, and polling reports what is live. Application and content are kept apart: content
 * changes refresh lists in place, while only a new app version offers a refresh of the page.
 */
class Versions {
  /** The newest content version `updates.json` reported; `''` before the first check. */
  latest = $state('');
  /** The app version `updates.json` last reported; `''` before the first check. */
  availableApp = $state('');

  /** The app version the page on screen was built with. */
  get app(): string {
    return page.model?.build.app ?? '';
  }

  /** The content version the page on screen was built from. */
  get content(): string {
    return page.model?.build.content ?? '';
  }

  /** A release is live that this page was not built with: the refresh pill shows. */
  get releasePending(): boolean {
    return this.availableApp !== '' && this.app !== '' && this.availableApp !== this.app;
  }
}

export const versions = new Versions();
