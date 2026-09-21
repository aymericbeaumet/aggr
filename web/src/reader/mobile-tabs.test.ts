import { expect, it, vi } from 'vitest';
import { createTouchTabActivation } from './mobile-tabs';

function harness() {
  const activate = vi.fn(), press = vi.fn();
  let time = 0;
  const taps = createTouchTabActivation<string>({ activate, press, now: () => time });
  return { taps, activate, press, advance: (ms: number) => { time += ms; } };
}

it('shows feedback on contact and activates on release without a delayed click', () => {
  const { taps, activate, press, advance } = harness();
  taps.start(1, 'browse', 100, 700, true);
  expect(press).toHaveBeenLastCalledWith('browse');
  expect(activate).not.toHaveBeenCalled();
  expect(taps.end(1, 'browse', 103, 702)).toBe(true);
  expect(activate).toHaveBeenCalledExactlyOnceWith('browse');
  expect(press).toHaveBeenLastCalledWith(null);
  expect(taps.consumeClick('browse', 0)).toBe(false);
  advance(300);
  expect(taps.consumeClick('browse', 1)).toBe(true);
  expect(taps.consumeClick('feed', 1)).toBe(false);
  advance(500);
  expect(taps.consumeClick('browse', 1)).toBe(false);
});

it('cancels drags, release outside the tab, pointer cancellation and multitouch', () => {
  for (const reason of ['drag', 'outside', 'cancel', 'multi'] as const) {
    const { taps, activate, press } = harness();
    taps.start(1, 'browse', 100, 700, true);
    if (reason === 'drag') taps.move(1, 100, 720);
    if (reason === 'cancel') taps.cancel();
    if (reason === 'multi') taps.start(2, 'feed', 200, 700, false);
    expect(taps.end(1, reason === 'outside' ? null : 'browse', 100, 700)).toBe(false);
    expect(activate).not.toHaveBeenCalled();
    expect(press).toHaveBeenLastCalledWith(null);
  }
});

it('ignores unrelated pointers and permits a subsequent deliberate tap or mouse click', () => {
  const { taps, activate } = harness();
  taps.start(1, 'browse', 100, 700, true);
  taps.move(7, 800, 900);
  expect(taps.end(7, 'browse', 100, 700)).toBe(false);
  expect(taps.end(1, 'browse', 100, 700)).toBe(true);
  taps.start(2, 'browse', 100, 700, true);
  expect(taps.end(2, 'browse', 100, 700)).toBe(true);
  expect(activate).toHaveBeenCalledTimes(2);
  taps.reset();
  expect(taps.consumeClick('browse', 1)).toBe(false);
});
