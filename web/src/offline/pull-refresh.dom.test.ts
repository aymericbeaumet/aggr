import { afterEach, beforeEach, describe, expect, it, testTools } from "../../test/browser";
import { installPullRefresh, pull } from './pull-refresh.svelte';

let events: ReturnType<typeof testTools.spyOn>;
let refresh: ReturnType<typeof testTools.fn<() => void>>;
let dispose: () => void;
let frames: Map<number, FrameRequestCallback>;
let frameId: number;

function frame(): void {
  const callbacks = [...frames.values()];
  frames.clear();
  for (const callback of callbacks) callback(0);
}

function touch(target: EventTarget, type: string, y: number, count = 1): Event {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(event, 'touches', { value: Array.from({ length: count }, () => ({ clientX: 100, clientY: y })) });
  Object.defineProperty(event, 'changedTouches', { value: [{ clientX: 100, clientY: y }] });
  target.dispatchEvent(event);
  return event;
}

beforeEach(() => {
  document.body.innerHTML = '<div id="pull-refresh"><svg class="pull-refresh-symbol"></svg></div><main><p>Article text to read</p></main>';
  pull.phase = 'idle';
  frames = new Map();
  frameId = 0;
  testTools.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  testTools.stubGlobal('matchMedia', testTools.fn(() => ({ matches: false })));
  testTools.spyOn(window, 'requestAnimationFrame').mockImplementation(callback => {
    frames.set(++frameId, callback);
    return frameId;
  });
  testTools.spyOn(window, 'cancelAnimationFrame').mockImplementation(id => { frames.delete(id); });
  events = testTools.spyOn(document, 'addEventListener');
  refresh = testTools.fn();
  dispose = installPullRefresh({ enabled: () => true, refresh });
});

afterEach(() => {
  dispose?.();
  for (const [name, listener, options] of events.mock.calls) document.removeEventListener(name, listener, options);
  document.getSelection()?.removeAllRanges();
  document.documentElement.removeAttribute('data-pull-state');
  document.documentElement.style.removeProperty('--pull-distance');
  delete document.documentElement.dataset.motion;
  testTools.restoreAllMocks();
  testTools.unstubAllGlobals();
  testTools.useRealTimers();
});

describe('installed pull-to-refresh', () => {
  it('refreshes a deliberate pull from ordinary prose', () => {
    const target = document.querySelector('p')!;
    touch(target, 'touchstart', 100);
    expect(touch(target, 'touchmove', 200).defaultPrevented).toBe(true);
    touch(target, 'touchend', 200, 0);
    expect(refresh).not.toHaveBeenCalled();
    frame();
    expect(refresh).not.toHaveBeenCalled();
    frame();
    expect(refresh).toHaveBeenCalledOnce();
  });

  it('coalesces movement into one local paint without restyling the page', () => {
    const target = document.querySelector('p')!;
    const indicator = document.getElementById('pull-refresh')!;
    const rootStyle = document.documentElement.getAttribute('style');
    touch(target, 'touchstart', 100);
    touch(target, 'touchmove', 130);
    touch(target, 'touchmove', 150);
    touch(target, 'touchmove', 170);
    expect(frames.size).toBe(1);
    expect(indicator.style.transform).toBe('');
    frame();
    expect(indicator.style.transform).toContain('translate3d');
    expect(document.documentElement.getAttribute('style')).toBe(rootStyle);
    expect(document.querySelector('main')!.style.transform).toBe('');
  });

  it('retracts a short pull before hiding the indicator', () => {
    const target = document.querySelector('p')!;
    touch(target, 'touchstart', 100);
    touch(target, 'touchmove', 150);
    frame();
    touch(target, 'touchend', 150, 0);
    expect(pull.phase).toBe('settling');
    expect(pull.hidden).toBe(false);
    frame();
    testTools.advanceTimersByTime(300);
    expect(pull.phase).toBe('idle');
    expect(pull.hidden).toBe(true);
    expect(refresh).not.toHaveBeenCalled();
  });

  it('does not leave a blocking touchmove handler installed between gestures', () => {
    expect(events.mock.calls.some((call: unknown[]) => call[0] === 'touchmove')).toBe(false);
    const remove = testTools.spyOn(document, 'removeEventListener');
    const target = document.querySelector('p')!;
    touch(target, 'touchstart', 100);
    expect(events.mock.calls.some((call: unknown[]) => call[0] === 'touchmove')).toBe(true);
    touch(target, 'touchend', 100, 0);
    expect(remove.mock.calls.some(([name]) => name === 'touchmove')).toBe(true);
  });

  it('disposes pending paints and refreshes when the page leaves', () => {
    const target = document.querySelector('p')!;
    touch(target, 'touchstart', 100);
    touch(target, 'touchmove', 200);
    touch(target, 'touchend', 200, 0);
    dispose();
    frame();
    frame();
    expect(refresh).not.toHaveBeenCalled();
    expect(pull.phase).toBe('idle');
  });

  it.each(['system', 'preference'])('settles immediately when %s motion is disabled', (mode) => {
    if (mode === 'system') testTools.mocked(window.matchMedia).mockReturnValue({ matches: true } as MediaQueryList);
    else document.documentElement.dataset.motion = 'off';
    const target = document.querySelector('p')!;
    touch(target, 'touchstart', 100);
    touch(target, 'touchmove', 150);
    frame();
    touch(target, 'touchend', 150, 0);
    expect(pull.phase).toBe('idle');
    expect(frames.size).toBe(0);
  });

  it('does not let an old settling deadline hide a new pull', () => {
    const target = document.querySelector('p')!;
    touch(target, 'touchstart', 100);
    touch(target, 'touchmove', 150);
    frame();
    touch(target, 'touchend', 150, 0);
    touch(target, 'touchstart', 100);
    touch(target, 'touchmove', 160);
    frame();
    testTools.advanceTimersByTime(300);
    expect(pull.phase).toBe('pulling');
    expect(pull.hidden).toBe(false);
  });

  it('retracts when the finger returns to the gesture slop', () => {
    const target = document.querySelector('p')!;
    touch(target, 'touchstart', 100);
    touch(target, 'touchmove', 200);
    frame();
    touch(target, 'touchmove', 103);
    frame();
    expect(document.getElementById('pull-refresh')!.style.opacity).toBe('0');
    touch(target, 'touchend', 103, 0);
    expect(pull.phase).toBe('idle');
    frame();
    frame();
    expect(refresh).not.toHaveBeenCalled();
  });

  it('uses the release position when a final move was not delivered', () => {
    const target = document.querySelector('p')!;
    touch(target, 'touchstart', 100);
    touch(target, 'touchmove', 200);
    frame();
    touch(target, 'touchend', 103, 0);
    expect(pull.phase).toBe('settling');
    frame();
    frame();
    expect(refresh).not.toHaveBeenCalled();
  });

  it('keeps disabled apps out of the blocking path', () => {
    dispose();
    dispose = installPullRefresh({ enabled: () => false, refresh });
    const target = document.querySelector('p')!;
    touch(target, 'touchstart', 100);
    expect(touch(target, 'touchmove', 200).defaultPrevented).toBe(false);
    expect(events.mock.calls.some((call: unknown[]) => call[0] === 'touchmove')).toBe(false);
  });

  it('keeps refreshing through extra touch events and calls refresh only once', () => {
    const target = document.querySelector('p')!;
    touch(target, 'touchstart', 100);
    touch(target, 'touchmove', 200);
    touch(target, 'touchend', 200, 0);
    touch(target, 'touchstart', 250);
    touch(target, 'touchend', 250, 0);
    touch(target, 'touchcancel', 250, 0);
    frame();
    frame();
    frame();
    expect(refresh).toHaveBeenCalledOnce();
    expect(pull.phase).toBe('refreshing');
  });

  it.each(['button', 'a href="/article/"', 'video', 'pre', 'div contenteditable="true"', 'div role="slider"', 'nav data-site-navigation'])(
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
    expect(['settling', 'idle']).toContain(pull.phase);
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
