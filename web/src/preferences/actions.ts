import { messages, preferences } from '../state/preferences.svelte';
import { decodeState, exportText, fragmentState, parseTransfer, shareLink, TEXT_LIMIT } from './transfer';

/**
 * The transfer buttons of `/preferences/`: the browser side of `transfer.ts`, loaded with the
 * form. Each one reports through `preferences.status`.
 */

/** The link that carries the current settings, under the site root. */
export function currentLink(root: string): string {
  return shareLink(root, preferences.values ?? {});
}

/** Copy the link; where the clipboard is out of reach, hand it to `offer` to show selected. */
export async function copyLink(root: string, offer: (url: string) => void): Promise<void> {
  const url = currentLink(root);
  const fallback = () => {
    offer(url);
    preferences.status = messages.offered;
  };
  if (!navigator.clipboard) return fallback();
  try {
    await navigator.clipboard.writeText(url);
    preferences.status = messages.copied;
  } catch {
    fallback();
  }
}

export async function share(root: string): Promise<void> {
  try {
    await navigator.share({ title: 'aggr preferences', url: currentLink(root) });
  } catch (error) {
    if (!(error instanceof Error && error.name === 'AbortError')) preferences.status = messages.unshareable;
  }
}

/** Download the settings as `aggr-preferences.json`. */
export function saveFile(doc: Document = document): void {
  const blob = new Blob([exportText(preferences.values ?? {})], { type: 'application/json' });
  const url = URL.createObjectURL(blob);
  const link = doc.createElement('a');
  link.href = url;
  link.download = 'aggr-preferences.json';
  doc.body.appendChild(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
  preferences.status = messages.savedFile;
}

/** Read a chosen file into the review; anything but a valid envelope changes nothing. */
export async function importFile(file: File): Promise<void> {
  try {
    if (file.size > TEXT_LIMIT) throw new Error('File too large');
    preferences.review(parseTransfer(preferences.schema, await file.text()));
  } catch {
    preferences.cancel(messages.invalidFile);
  }
}

/**
 * A shared link carries its payload in the fragment, so it never reaches a server log; it is
 * taken out of the address before anything else, so a reload or a copied address does not
 * carry it again. The settings go to the review, never straight into storage.
 */
export function importFragment(win: Window = window): void {
  const encoded = fragmentState(win.location.hash);
  if (encoded === null) return;
  const url = new URL(win.location.href);
  url.hash = '';
  win.history.replaceState(win.history.state, '', url.href);
  try {
    preferences.review(parseTransfer(preferences.schema, decodeState(encoded)));
  } catch {
    preferences.cancel(messages.invalidLink);
  }
}
