import type { Attachment } from 'svelte/attachments';
import type { ClientRow } from '@aggr-model/ClientRow';
import { navigation } from '../navigation';
import { page } from '../state/page.svelte';
import { isInstalled, markExternalLinks } from './external-links';
import { installFootnotes } from './footnotes';
import { installHeadings } from './headings';
import { neighborDestination } from './neighbors';
import { installSelectionShare } from './selection-share';
import { installSwipes } from './swipes';

/**
 * Article-local behavior belongs to the component's lifetime. Sanitized prose remains opaque;
 * the media controller loads only when the article has a player to drive.
 */

export type EnhanceOptions = {
  next: ClientRow | null;
  previous: ClientRow | null;
};

/** What the media chunk knows how to drive. */
export const MEDIA = '[data-audio-component], .native-video video, [data-video-embed]';

/** Never let one broken enhancement take the rest of the article down with it. */
function safely(label: string, run: () => void): void {
  try {
    run();
  } catch (error) {
    console.error(`aggr: ${label}`, error);
  }
}

export function enhance(options: EnhanceOptions): Attachment<HTMLElement> {
  return (node) => {
    const controller = new AbortController();
    const { signal } = controller;
    const root = page.root || document.baseURI;
    safely('footnotes', () => installFootnotes(node, signal));
    safely('headings', () => installHeadings(node, signal));
    safely('external-links', () => markExternalLinks(node, root, isInstalled()));
    safely('selection-share', () => {
      const body = node.querySelector('.body');
      if (body instanceof HTMLElement) installSelectionShare(body, signal);
    });
    safely('swipes', () =>
      installSwipes(node, signal, (direction) => {
        void navigation.go(neighborDestination(root, direction === 1 ? options.next : options.previous));
      }),
    );
    if (node.querySelector(MEDIA)) {
      import('../media')
        .then(({ mount }) => {
          // The page may have been left while the chunk was on its way.
          if (!signal.aborted) mount(node, signal);
        })
        .catch((error: unknown) => console.error('aggr: media', error));
    }
    return () => controller.abort();
  };
}
