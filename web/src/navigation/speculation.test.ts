import { describe, expect, it } from 'vitest';
import { Speculation, frugal } from './speculation';

describe('Speculation', () => {
  it('admits sixteen new guesses per screen and any known page', () => {
    const speculation = new Speculation();
    for (let index = 0; index < 16; index += 1) expect(speculation.admit(false)).toBe(true);
    expect(speculation.admit(false)).toBe(false);
    expect(speculation.admit(true)).toBe(true);
    speculation.reset();
    expect(speculation.admit(false)).toBe(true);
    expect(speculation.count).toBe(1);
  });
});

describe('frugal', () => {
  it('reads Save-Data and 2G class connections', () => {
    expect(frugal(undefined)).toBe(false);
    expect(frugal({ saveData: true })).toBe(true);
    expect(frugal({ effectiveType: 'slow-2g' })).toBe(true);
    expect(frugal({ effectiveType: '2g' })).toBe(true);
    expect(frugal({ effectiveType: '4g' })).toBe(false);
  });
});
