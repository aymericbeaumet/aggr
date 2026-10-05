/**
 * The tab a press has chosen before its page is on screen. The header prefers this over the
 * page kind, so a tap does not wait for the fetch to look selected. Cleared when the page
 * arrives or the press is abandoned.
 */
class TabIntent {
  route = $state<string | null>(null);
}

export const tabIntent = new TabIntent();
