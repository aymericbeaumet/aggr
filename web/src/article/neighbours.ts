import { sitePath } from '../model/urls';

/**
 * Where turning the page goes: the neighbouring article, or the main feed past either end of
 * the archive. The keyboard and the swipes share this so they can never disagree.
 */
export function neighbourDestination(root: string, neighbour: { url: string } | null | undefined): string {
  return new URL(neighbour ? sitePath(neighbour.url) : '', root).href;
}
