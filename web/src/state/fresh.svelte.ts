/**
 * The entries that arrived since this browsing session last saw the feed. `pending` holds the
 * ones nobody has been shown yet (absolute item URLs, newest first) and `marked` the ones lit
 * as new on the list on screen; both are tab-local, persisted in `sessionStorage` by
 * `updates/badges.ts`. Empty until the reader runs, so a server render lights nothing.
 */
class Fresh {
  pending = $state.raw<string[]>([]);
  marked = $state.raw<string[]>([]);
  /** Whether the document is visible: entries are only marked in front of someone. */
  visible = $state(true);
}

export const fresh = new Fresh();
