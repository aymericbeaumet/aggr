import { expect, it } from 'vitest';
import { searchInputVisible } from './focus';

const viewport = { top: 44, bottom: 787, left: 0, right: 390 };
const input = { top: 66, bottom: 110, left: 12, right: 378 };
it('keeps fully visible search inputs in place, including exact viewport boundaries', () => {
  expect(searchInputVisible(input, viewport)).toBe(true);
  expect(searchInputVisible(viewport, viewport)).toBe(true);
});
it('detects clipping by sticky headers, the keyboard, bottom navigation and zoomed viewport edges', () => {
  expect(searchInputVisible({ ...input, top: 40 }, viewport)).toBe(false);
  expect(searchInputVisible({ ...input, top: -30, bottom: 14 }, viewport)).toBe(false);
  expect(searchInputVisible({ ...input, top: 770, bottom: 814 }, viewport)).toBe(false);
  expect(searchInputVisible(input, { ...viewport, bottom: 100 })).toBe(false);
  expect(searchInputVisible(input, { ...viewport, left: 20 })).toBe(false);
  expect(searchInputVisible(input, { ...viewport, right: 370 })).toBe(false);
  expect(searchInputVisible(input, { ...viewport, top: 120 })).toBe(false);
});
