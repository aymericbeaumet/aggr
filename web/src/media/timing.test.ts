import { describe, expect, it } from 'vitest';
import { clock, duration, endsAt, wallClock } from './timing';

describe('duration', () => {
  it('accepts only finite positive lengths', () => {
    expect(duration(600)).toBe(600);
    expect(duration(0)).toBeUndefined();
    expect(duration(-1)).toBeUndefined();
    expect(duration(Infinity)).toBeUndefined();
    expect(duration(NaN)).toBeUndefined();
    expect(duration('600')).toBeUndefined();
    expect(duration(Number.MAX_SAFE_INTEGER + 1)).toBeUndefined();
  });
});

describe('clock', () => {
  it('formats minutes, hours and the impossible', () => {
    expect(clock(0)).toBe('0:00');
    expect(clock(65)).toBe('1:05');
    expect(clock(3599)).toBe('59:59');
    expect(clock(3600)).toBe('1:00:00');
    expect(clock(3661.9)).toBe('1:01:01');
    expect(clock(NaN)).toBe('—');
    expect(clock(-5)).toBe('—');
  });
});

describe('endsAt', () => {
  const now = 1_700_000_000_000;

  it('accounts for the playback speed while playing', () => {
    const estimate = endsAt({ phase: 'playing', position: 100, duration: 700, speed: 2 }, now);
    expect(estimate.remaining).toBe(300);
    expect(estimate.endsAt).toBe(now + 300_000);
  });

  it('keeps the remaining time but promises no finish while paused, loading or ended', () => {
    for (const phase of ['paused', 'loading', 'idle', 'error'] as const) {
      const estimate = endsAt({ phase, position: 100, duration: 700, speed: 1 }, now);
      expect(estimate.remaining).toBe(600);
      expect(estimate.endsAt).toBeUndefined();
    }
    expect(endsAt({ phase: 'ended', position: 700, duration: 700, speed: 1 }, now)).toEqual({
      remaining: 0,
      endsAt: undefined,
    });
  });

  it('has nothing to say for live, unknown or unmeasured playback', () => {
    expect(endsAt({ phase: 'playing', position: 10, speed: 1 }, now)).toEqual({});
    expect(endsAt({ phase: 'playing', position: 10, duration: 100, speed: NaN }, now)).toEqual({});
    expect(endsAt({ phase: 'playing', position: NaN, duration: 100, speed: 1 }, now)).toEqual({});
    expect(endsAt({ phase: 'playing', position: 10, duration: Infinity, speed: 1 }, now)).toEqual({});
  });

  it('never runs backwards past the end or before the start', () => {
    expect(endsAt({ phase: 'playing', position: 900, duration: 700, speed: 1 }, now).remaining).toBe(0);
    expect(endsAt({ phase: 'playing', position: -30, duration: 700, speed: 1 }, now).remaining).toBe(700);
  });
});

describe('wallClock', () => {
  it('shows hours and minutes on the local clock', () => {
    const at = new Date(2026, 8, 30, 7, 5).getTime();
    expect(wallClock(at)).toBe('07:05');
    expect(wallClock(NaN)).toBe('');
  });
});
