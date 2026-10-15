import { beforeAll, beforeEach, describe, expect, it } from 'vitest';
import type { CursorRow } from '../selection/cursor';
import { selection } from '../state/selection.svelte';
import { installShortcuts } from './index';
import { resetModifiers } from './keys';

function row(path: string): CursorRow {
  return {
    path,
    href: `https://reader.test/${path}/`,
    link: `https://example.test/${path}`,
    title: path,
    original: null,
    discussions: [],
  };
}

function press(target: EventTarget, key: string, extra: KeyboardEventInit = {}): KeyboardEvent {
  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...extra });
  target.dispatchEvent(event);
  return event;
}

beforeAll(() => {
  Element.prototype.scrollIntoView = () => {};
  installShortcuts();
});

beforeEach(() => {
  resetModifiers();
  document.body.replaceChildren();
  const list = document.createElement('ul');
  list.className = 'rows';
  for (const path of ['items/a', 'items/b']) {
    const item = document.createElement('li');
    item.className = 'row';
    item.dataset.path = path;
    const link = document.createElement('a');
    link.href = `/${path}/`;
    link.dataset.rowOpen = '';
    link.textContent = path;
    item.append(link);
    list.append(item);
  }
  const field = document.createElement('input');
  field.id = 'q';
  field.type = 'search';
  field.setAttribute('role', 'combobox');
  const speed = document.createElement('select');
  speed.dataset.audioSpeed = '';
  speed.innerHTML = '<option value="1">1×</option><option value="1.5">1.5×</option>';
  const seek = document.createElement('input');
  seek.type = 'range';
  const video = document.createElement('video');
  document.body.append(list, field, speed, seek, video);
  selection.sync([row('items/a'), row('items/b')], location.href);
});

describe('installShortcuts', () => {
  it('moves the list from a playback select and leaves its arrows alone', () => {
    const speed = document.querySelector('select');
    if (!(speed instanceof HTMLSelectElement)) throw new Error('missing select');
    const stepped = press(speed, 'j');
    expect(stepped.defaultPrevented).toBe(true);
    expect(selection.url).toBe('https://reader.test/items/b/');

    selection.sync([row('items/a'), row('items/b')], location.href);
    const seek = document.querySelector('input[type="range"]');
    if (!(seek instanceof HTMLInputElement)) throw new Error('missing range');
    const arrow = press(seek, 'ArrowDown');
    expect(arrow.defaultPrevented).toBe(false);
    expect(selection.url).toBe('https://reader.test/items/a/');
  });

  it('leaves a text field to the reader and still handles j over a video key', () => {
    const field = document.getElementById('q');
    if (!(field instanceof HTMLInputElement)) throw new Error('missing search');
    const typed = press(field, 'j');
    expect(typed.defaultPrevented).toBe(false);
    expect(selection.url).toBe('https://reader.test/items/a/');

    const video = document.querySelector('video');
    if (!(video instanceof HTMLVideoElement)) throw new Error('missing video');
    video.addEventListener('keydown', (event) => event.preventDefault());
    const stepped = press(video, 'j');
    expect(stepped.defaultPrevented).toBe(true);
    expect(selection.url).toBe('https://reader.test/items/b/');
  });

  it('treats a modifier flag as stuck after blur, and a real chord still reaches search', () => {
    const blocked = press(document.body, 'j', { metaKey: true });
    expect(blocked.defaultPrevented).toBe(false);
    expect(selection.url).toBe('https://reader.test/items/a/');

    window.dispatchEvent(new Event('blur'));
    const stuck = press(document.body, 'j', { metaKey: true });
    expect(stuck.defaultPrevented).toBe(true);
    expect(selection.url).toBe('https://reader.test/items/b/');

    selection.sync([row('items/a'), row('items/b')], location.href);
    const stuckSearch = press(document.body, 'k', { metaKey: true });
    expect(document.activeElement?.id).not.toBe('q');
    expect(stuckSearch.defaultPrevented).toBe(true);

    press(document.body, 'Meta', { metaKey: true });
    const chord = press(document.body, 'k', { metaKey: true });
    expect(chord.defaultPrevented).toBe(true);
    expect(document.activeElement?.id).toBe('q');
  });

  it('scrolls on ctrl+e from a fresh page, and again after blur once control goes down', () => {
    const tops: number[] = [];
    const original = window.scrollBy;
    window.scrollBy = ((options?: ScrollToOptions) => {
      tops.push(options?.top ?? 0);
    }) as typeof window.scrollBy;
    try {
      expect(press(document.body, 'e', { ctrlKey: true }).defaultPrevented).toBe(true);
      window.dispatchEvent(new Event('blur'));
      expect(press(document.body, 'e', { ctrlKey: true }).defaultPrevented).toBe(false);
      press(document.body, 'Control', { ctrlKey: true });
      expect(press(document.body, 'e', { ctrlKey: true }).defaultPrevented).toBe(true);
      expect(tops).toHaveLength(2);
    } finally {
      window.scrollBy = original;
    }
  });
});
