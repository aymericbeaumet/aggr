import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { installPullRefresh, pull } from './pull-refresh.svelte';

let events: ReturnType<typeof vi.spyOn>;
let refresh: ReturnType<typeof vi.fn<() => void>>;

function touch(target: EventTarget, type: string, y: number, count = 1): Event {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(event, 'touches', { value: Array.from({ length: count }, () => ({ clientX: 100, clientY: y })) });
  target.dispatchEvent(event);
  return event;
}

beforeEach(() => {
  document.body.innerHTML = '<main><p>Article text to read</p></main>';
  pull.phase = 'idle';
  events = vi.spyOn(document, 'addEventListener');
  refresh = vi.fn();
  installPullRefresh({ enabled: () => true, refresh });
});

afterEach(() => {
  for (const [name, listener, options] of events.mock.calls) document.removeEventListener(name, listener, options);
  document.getSelection()?.removeAllRanges();
  document.documentElement.removeAttribute('data-pull-state');
  document.documentElement.style.removeProperty('--pull-distance');
  vi.restoreAllMocks();
});

describe('installed pull-to-refresh', () => {
  it('refreshes a deliberate pull from ordinary prose', () => {
    const target = document.querySelector('p')!;
    touch(target, 'touchstart', 100);
    expect(touch(target, 'touchmove', 200).defaultPrevented).toBe(true);
    touch(target, 'touchend', 200, 0);
    expect(refresh).toHaveBeenCalledOnce();
  });

  it.each(['button', 'a href="/article/"', 'video', 'pre', 'div contenteditable="true"', 'div role="slider"'])(
    'leaves a %s gesture alone', (markup) => {
      document.body.innerHTML = `<${markup}>Control</${markup.split(' ')[0]}>`;
      const target = document.body.firstElementChild!;
      touch(target, 'touchstart', 100);
      expect(touch(target, 'touchmove', 200).defaultPrevented).toBe(false);
      touch(target, 'touchend', 200, 0);
      expect(refresh).not.toHaveBeenCalled();
    },
  );

  it('leaves nested scrolling to the suggestion list, including at its top', () => {
    document.body.innerHTML = '<ul style="overflow-y:auto"><li>Suggestion</li></ul>';
    const list = document.querySelector('ul')!;
    Object.defineProperties(list, { scrollHeight: { value: 300 }, clientHeight: { value: 100 } });
    const option = document.querySelector('li')!;
    touch(option, 'touchstart', 100);
    expect(touch(option, 'touchmove', 200).defaultPrevented).toBe(false);
    touch(option, 'touchend', 200, 0);
    expect(refresh).not.toHaveBeenCalled();
  });

  it('cancels when a second finger arrives before release', () => {
    const target = document.querySelector('p')!;
    touch(target, 'touchstart', 100);
    touch(target, 'touchmove', 200);
    touch(target, 'touchstart', 200, 2);
    touch(target, 'touchend', 200, 1);
    expect(refresh).not.toHaveBeenCalled();
    expect(pull.phase).toBe('idle');
  });

  it.each(['before', 'during'])('preserves a text selection made %s the gesture', (when) => {
    const target = document.querySelector('p')!;
    const select = () => {
      const range = document.createRange();
      range.selectNodeContents(target);
      document.getSelection()?.addRange(range);
    };
    if (when === 'before') select();
    touch(target, 'touchstart', 100);
    if (when === 'during') select();
    expect(touch(target, 'touchmove', 200).defaultPrevented).toBe(false);
    touch(target, 'touchend', 200, 0);
    expect(refresh).not.toHaveBeenCalled();
  });
});
