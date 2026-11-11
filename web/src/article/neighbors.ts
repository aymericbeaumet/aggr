import { sitePath } from '../model/urls';

/**
 * Where turning the page goes: the neighboring article, or the main feed past either end of
 * the archive. The keyboard and the swipes share this so they can never disagree.
 */
export function neighborDestination(root: string, neighbor: { url: string } | null | undefined): string {
  return new URL(neighbor ? sitePath(neighbor.url) : '', root).href;
}
