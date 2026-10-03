import { describe, expect, it } from 'vitest';
import { swipeOutcome, touchCapable, vertical } from './swipes';

describe('touchCapable', () => {
  it('needs a touch point to swipe with', () => {
    expect(touchCapable({ maxTouchPoints: 5 })).toBe(true);
    expect(touchCapable({ maxTouchPoints: 0 })).toBe(false);
    expect(touchCapable({ maxTouchPoints: undefined as unknown as number })).toBe(false);
  });
});

describe('swipeOutcome', () => {
  it('turns to the next article on a quick swipe left and the previous on a swipe right', () => {
    expect(swipeOutcome(-120, 4, 300)).toBe(1);
    expect(swipeOutcome(150, -10, 500)).toBe(-1);
  });

  it('ignores short, slow, drifting or diagonal movements', () => {
    expect(swipeOutcome(-60, 0, 200)).toBe(0);
    expect(swipeOutcome(-200, 0, 1200)).toBe(0);
    expect(swipeOutcome(-200, 60, 200)).toBe(0);
    expect(swipeOutcome(-80, 45, 200)).toBe(0);
    expect(swipeOutcome(0, 0, 0)).toBe(0);
  });
});

describe('vertical', () => {
  it('recognises a scroll once the finger has clearly moved up or down', () => {
    expect(vertical(-10, -320)).toBe(true);
    expect(vertical(5, 13)).toBe(true);
    expect(vertical(40, 10)).toBe(false);
    expect(vertical(0, 8)).toBe(false);
  });
});
