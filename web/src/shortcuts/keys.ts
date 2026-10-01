import type { DiscussionLinkCtx } from '../generated/DiscussionLinkCtx';

/** Whether a key event belongs to an editor: shortcuts never fire while typing. */
export function editing(target: EventTarget | null): boolean {
  return (
    target instanceof Element &&
    (target.closest("input, textarea, select, [contenteditable]:not([contenteditable='false'])") !== null ||
      ['textbox', 'combobox', 'searchbox'].includes(target.getAttribute('role') || ''))
  );
}

/** The article the external shortcuts act on: the one on screen, or the row under the cursor. */
export type Subject = {
  original: string | null;
  link: string;
  title: string;
  discussions: Array<{ name: string; href: string }>;
};

/** `O` opens the original; a network's upper-case key opens its discussion, or its search. */
export function externalTarget(key: string, subject: Subject | null, networks: DiscussionLinkCtx[]): string | null {
  if (!subject) return null;
  if (key === 'O') return subject.original;
  const network = networks.find((candidate) => candidate.shortcut === key);
  if (!network) return null;
  const rendered = subject.discussions.find((link) => link.name === network.name);
  return rendered
    ? rendered.href
    : network.url
        .split('{url}')
        .join(encodeURIComponent(subject.link))
        .split('{title}')
        .join(encodeURIComponent(subject.title));
}

export type GotoTarget = { top: true } | { url: string } | null;

const ROUTES: Record<string, string> = { f: '', i: '', b: 'browse/', l: 'browse/', p: 'preferences/' };

/** The `g` chord: `gg` to the top, letters to routes, digits to the numbered entries. */
export function gotoTarget(key: string, root: string, entries: string[]): GotoTarget {
  if (key === 'g') return { top: true };
  if (Object.prototype.hasOwnProperty.call(ROUTES, key)) return { url: new URL(ROUTES[key], root).href };
  if (/^[1-9]$/.test(key)) {
    const entry = entries[Number(key) - 1];
    if (entry) return { url: new URL(entry, root).href };
  }
  return null;
}

/** How far `d`/`u` and Ctrl+e/y move: `amount` lines, capped at half of what is visible. */
export function scrollDistance(line: number, visible: number, amount: number, byLine: boolean): number {
  return byLine ? line : Math.min(line * amount, Math.max(line, visible) * 0.5);
}
