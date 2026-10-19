import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import Completions from './Completions.svelte';
import { search, type SearchDriver } from '../state/search.svelte';

let component: ReturnType<typeof mount> | undefined;
const choose = vi.fn();
const highlight = vi.fn();
let viewport: EventTarget & { height: number; offsetTop: number };

function pointer(target: Element, type: string, pointerType = 'touch', x = 20, y = 20): Event {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y });
  Object.defineProperties(event, {
    pointerType: { value: pointerType },
    pointerId: { value: 1 },
    isPrimary: { value: true },
  });
  target.dispatchEvent(event);
  return event;
}

beforeEach(() => {
  choose.mockClear();
  highlight.mockClear();
  viewport = Object.assign(new EventTarget(), { height: 800, offsetTop: 0 });
  vi.stubGlobal('visualViewport', viewport);
  document.body.innerHTML = '<input type="search"><div id="suggestions"></div>';
  document.querySelector('input')!.focus();
  search.open = true;
  search.highlighted = 0;
  search.completions = ['one', 'two'].map(id => ({ id, label: id, detail: '', insert: id, start: 0, end: 7 }));
  search.driver = { choose, highlight } as unknown as SearchDriver;
  component = mount(Completions, { target: document.getElementById('suggestions')! });
  flushSync();
});

afterEach(async () => {
  if (component) await unmount(component);
  search.driver = null;
  search.open = false;
  document.body.replaceChildren();
  vi.unstubAllGlobals();
});

describe('suggestion pointer gestures', () => {
  it('chooses a touch only when released and keeps the search field focused', () => {
    const option = document.querySelector('.search-completion')!;
    pointer(option, 'pointerdown');
    expect(choose).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(document.querySelector('input'));
    pointer(option, 'pointerup');
    expect(choose).toHaveBeenCalledExactlyOnceWith(search.completions[0]);
  });

  it('leaves a touch drag to scrolling without choosing or moving the keyboard highlight', () => {
    const option = document.querySelector('.search-completion')!;
    pointer(option, 'pointerdown');
    pointer(option, 'pointermove', 'touch', 20, 60);
    pointer(option, 'pointerup', 'touch', 20, 60);
    expect(choose).not.toHaveBeenCalled();
    expect(highlight).not.toHaveBeenCalled();
  });

  it('never chooses a touch cancelled by native scrolling', () => {
    const option = document.querySelector('.search-completion')!;
    pointer(option, 'pointerdown');
    pointer(option, 'pointercancel');
    pointer(option, 'pointerup');
    expect(choose).not.toHaveBeenCalled();
  });

  it('rejects a release that moved even when no pointermove was delivered', () => {
    const option = document.querySelector('.search-completion')!;
    pointer(option, 'pointerdown');
    pointer(option, 'pointerup', 'touch', 20, 60);
    expect(choose).not.toHaveBeenCalled();
  });

  it('keeps mouse selection on press and hover highlighting', () => {
    const option = document.querySelectorAll('.search-completion')[1];
    expect(pointer(option, 'pointerdown', 'mouse').defaultPrevented).toBe(true);
    pointer(option, 'pointerup', 'mouse');
    expect(choose).toHaveBeenCalledExactlyOnceWith(search.completions[1]);
    pointer(option, 'pointermove', 'mouse');
    expect(highlight).toHaveBeenCalledExactlyOnceWith(1);
  });
});

describe('suggestion viewport bounds', () => {
  it('fits below the input while the keyboard shrinks or pans the visual viewport', () => {
    const list = document.querySelector<HTMLElement>('.search-completions')!;
    list.getBoundingClientRect = () => new DOMRect(0, 100, 320, 400);
    viewport.height = 360;
    viewport.offsetTop = 20;
    viewport.dispatchEvent(new Event('resize'));
    flushSync();
    expect(list.style.maxHeight).toBe('min(24rem, 272px)');

    viewport.offsetTop = 50;
    viewport.dispatchEvent(new Event('scroll'));
    flushSync();
    expect(list.style.maxHeight).toBe('min(24rem, 302px)');

    viewport.height = 40;
    viewport.dispatchEvent(new Event('resize'));
    flushSync();
    expect(list.style.maxHeight).toBe('min(24rem, 0px)');
  });

  it('stops watching the viewport when suggestions hide and when the component unmounts', async () => {
    const remove = vi.spyOn(viewport, 'removeEventListener');
    search.open = false;
    flushSync();
    expect(remove).toHaveBeenCalledWith('resize', expect.any(Function));
    expect(remove).toHaveBeenCalledWith('scroll', expect.any(Function));
    remove.mockClear();
    search.open = true;
    flushSync();
    if (component) await unmount(component);
    component = undefined;
    expect(remove).toHaveBeenCalledWith('resize', expect.any(Function));
    expect(remove).toHaveBeenCalledWith('scroll', expect.any(Function));
  });
});
