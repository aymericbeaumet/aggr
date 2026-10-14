import type { DiscussionLinkCtx } from '../generated/DiscussionLinkCtx';

const TEXT_ENTRY = new Set(['text', 'search', 'url', 'email', 'password', 'tel', 'number']);

/**
 * Whether a key event belongs to text entry. Shortcuts never fire while typing. A slider, a
 * select, or a player is not text entry: letter keys still belong to the reader.
 */
export function editing(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  const editable = target.closest('[contenteditable]');
  if (editable instanceof HTMLElement && editable.getAttribute('contenteditable')?.toLowerCase() !== 'false') return true;
  const field = target.closest('input, textarea, [role="textbox"], [role="searchbox"], [role="combobox"]');
  if (field instanceof HTMLTextAreaElement) return true;
  if (field instanceof HTMLInputElement) return TEXT_ENTRY.has(field.type);
  const role = field?.getAttribute('role') ?? '';
  return role === 'textbox' || role === 'searchbox' || role === 'combobox';
}

/** Arrow keys move a slider, a select, or native playback. Letter shortcuts stay with the reader. */
export function controlOwnsArrows(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  const control = target.closest('input, select, video, audio');
  if (
    control instanceof HTMLSelectElement ||
    control instanceof HTMLVideoElement ||
    control instanceof HTMLAudioElement
  ) {
    return true;
  }
  return control instanceof HTMLInputElement && control.type === 'range';
}

const modifiers = { meta: false, ctrl: false, alt: false };
// A fresh page trusts the event flags, so a chord works before any modifier key has been seen.
// Once a modifier is released, or the window blurs, a later flag is stuck until that key goes down again.
let trustModifiers = true;

function modifierName(event: KeyboardEvent): 'meta' | 'ctrl' | 'alt' | null {
  if (event.key === 'Meta') return 'meta';
  if (event.key === 'Control') return 'ctrl';
  if (event.key === 'Alt') return 'alt';
  return null;
}

/** Record a modifier key going down or up. A letter whose flag is merely stuck does not count. */
export function noteModifier(event: KeyboardEvent, pressed: boolean): void {
  const name = modifierName(event);
  if (!name) return;
  modifiers[name] = pressed;
  trustModifiers = modifiers.meta || modifiers.ctrl || modifiers.alt;
}

/** A blur delivers the matching keyup somewhere else. The next letter is not a chord. */
export function noteModifierBlur(): void {
  modifiers.meta = false;
  modifiers.ctrl = false;
  modifiers.alt = false;
  trustModifiers = false;
}

/** Back to a fresh page: event flags count again. Tests start here. */
export function resetModifiers(): void {
  modifiers.meta = false;
  modifiers.ctrl = false;
  modifiers.alt = false;
  trustModifiers = true;
}

/** Which modifier flags should block a shortcut. Empty after a blur or a release. */
export function modifiersHeld(event: KeyboardEvent): { meta: boolean; ctrl: boolean; alt: boolean } {
  if (!trustModifiers) return { meta: false, ctrl: false, alt: false };
  return { meta: event.metaKey, ctrl: event.ctrlKey, alt: event.altKey };
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
