import { afterEach, describe, expect, it, testTools } from "../../test/browser";
import { installFeedChrome, installKeyboardInset } from './platform';

/** Deliver resize observations explicitly so the measurement sequence is deterministic. */
let observer: { observed: Element[]; disconnected: boolean; fire: () => void } | null = null;

function installFakeObserver(): void {
  observer = null;
  globalThis.ResizeObserver = class {
    observed: Element[] = [];
    disconnected = false;
    constructor(private readonly notify: () => void) {
      observer = this;
    }
    observe(element: Element): void {
      this.observed.push(element);
    }
    unobserve(): void {}
    disconnect(): void {
      this.disconnected = true;
    }
    fire(): void {
      this.notify();
    }
  } as unknown as typeof ResizeObserver;
}

function height(element: HTMLElement, value: number): void {
  element.getBoundingClientRect = () => new DOMRect(0, 0, 320, value);
}

describe('installFeedChrome', () => {
  const original = globalThis.ResizeObserver;

  afterEach(() => {
    globalThis.ResizeObserver = original;
    document.documentElement.style.removeProperty('--top-nav-offset');
    document.documentElement.style.removeProperty('--feed-heading');
    document.body.replaceChildren();
    observer = null;
  });

  it('measures the header and a scoped title, then clears a title the next page does not have', () => {
    installFakeObserver();
    document.body.innerHTML = '<header class="top"></header><div class="listhead"></div>';
    const header = document.querySelector('.top');
    const heading = document.querySelector('.listhead');
    if (!(header instanceof HTMLElement) || !(heading instanceof HTMLElement)) throw new Error('fixture');
    height(header, 72.5);
    height(heading, 40);
    const first = new AbortController();
    installFeedChrome(first.signal);
    expect(document.documentElement.style.getPropertyValue('--top-nav-offset')).toBe('72.5px');
    expect(document.documentElement.style.getPropertyValue('--feed-heading')).toBe('40px');
    expect(observer?.observed).toEqual([header, heading]);

    height(heading, 56);
    observer?.fire();
    expect(document.documentElement.style.getPropertyValue('--feed-heading')).toBe('56px');

    first.abort();
    expect(observer?.disconnected).toBe(true);
    document.documentElement.style.setProperty('--feed-heading', '1px');
    observer?.fire();
    expect(document.documentElement.style.getPropertyValue('--feed-heading')).toBe('1px');

    document.body.innerHTML = '<header class="top"></header>';
    const next = document.querySelector('.top');
    if (!(next instanceof HTMLElement)) throw new Error('fixture');
    height(next, 44);
    installFeedChrome(new AbortController().signal);
    expect(document.documentElement.style.getPropertyValue('--top-nav-offset')).toBe('44px');
    expect(document.documentElement.style.getPropertyValue('--feed-heading')).toBe('0px');
  });

  it('leaves the header offset alone when the header has no height yet', () => {
    installFakeObserver();
    document.documentElement.style.setProperty('--top-nav-offset', '44px');
    document.body.innerHTML = '<header class="top"></header>';
    const header = document.querySelector('.top');
    if (!(header instanceof HTMLElement)) throw new Error('fixture');
    height(header, 0);
    installFeedChrome(new AbortController().signal);
    expect(document.documentElement.style.getPropertyValue('--top-nav-offset')).toBe('44px');
    expect(document.documentElement.style.getPropertyValue('--feed-heading')).toBe('0px');
  });
});

describe('keyboard inset', () => {
  afterEach(() => {
    document.documentElement.classList.remove('is-keyboard-open');
    document.body.replaceChildren();
    testTools.unstubAllGlobals();
    testTools.restoreAllMocks();
  });

  it('distinguishes pinch zoom from the keyboard and restores tabs when editing ends', async () => {
    const events = testTools.spyOn(document, 'addEventListener');
    const viewport = Object.assign(new EventTarget(), { height: 400, scale: 2 });
    testTools.stubGlobal('visualViewport', viewport);
    testTools.stubGlobal('innerHeight', 800);
    document.body.innerHTML = '<input type="search">';
    const input = document.querySelector('input')!;
    input.focus();
    expect(document.activeElement).toBe(input);
    installKeyboardInset();
    try {
      expect(document.documentElement.classList.contains('is-keyboard-open')).toBe(false);
      viewport.height = 200;
      viewport.dispatchEvent(new Event('resize'));
      expect(document.documentElement.classList.contains('is-keyboard-open')).toBe(true);
      input.blur();
      await testTools.waitFor(() => expect(document.activeElement).not.toBe(input));
      expect(document.documentElement.classList.contains('is-keyboard-open')).toBe(false);
    } finally {
      for (const [name, listener, options] of events.mock.calls) document.removeEventListener(name, listener, options);
    }
  });
});
