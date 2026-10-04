import { describe, expect, it } from 'vitest';
import { frugal } from './speculation';

describe('frugal', () => {
  it('reads Save-Data and 2G class connections', () => {
    expect(frugal(undefined)).toBe(false);
    expect(frugal({ saveData: true })).toBe(true);
    expect(frugal({ effectiveType: 'slow-2g' })).toBe(true);
    expect(frugal({ effectiveType: '2g' })).toBe(true);
    expect(frugal({ effectiveType: '4g' })).toBe(false);
  });
});
