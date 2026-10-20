import { describe, expect, it } from 'vitest';
import { pullLabel, pullState } from './pull-refresh.svelte';

describe('pullState', () => {
  it('leaves a gesture that is not tracking alone', () => {
    expect(pullState('idle', 0, 40)).toEqual({ phase: 'idle', distance: 0 });
    expect(pullState('refreshing', 0, 40)).toEqual({ phase: 'refreshing', distance: 0 });
  });

  it('gives up on a move up or sideways', () => {
    expect(pullState('tracking', 0, 0)).toEqual({ phase: 'idle', distance: 0 });
    expect(pullState('pulling', 0, -4)).toEqual({ phase: 'idle', distance: 0 });
    expect(pullState('tracking', 30, 20)).toEqual({ phase: 'idle', distance: 0 });
  });

  it('waits for a deliberate pull, then follows the finger with give', () => {
    expect(pullState('tracking', 0, 5)).toEqual({ phase: 'tracking', distance: 0 });
    expect(pullState('tracking', 0, 6)).toEqual({ phase: 'pulling', distance: 0 });
    expect(pullState('pulling', 0, 40).distance).toBeCloseTo(23.06, 1);
    expect(pullState('pulling', 0, 84).phase).toBe('armed');
    expect(pullState('armed', 0, 200).distance).toBeCloseTo(68.50, 1);
    expect(pullState('armed', 0, 60).phase).toBe('pulling');
  });

  it('keeps fractional movement continuous and adds resistance without a hard stop', () => {
    const samples = [6, 6.1, 40, 40.1, 200, 200.1, 1000].map(y => pullState('pulling', 0, y).distance);
    expect(samples[1]).toBeGreaterThan(0);
    expect(samples[3]).toBeGreaterThan(samples[2]);
    expect(samples[3] - samples[2]).toBeLessThan(0.1);
    expect(samples[5]).toBeGreaterThan(samples[4]);
    expect(samples[5] - samples[4]).toBeLessThan(samples[3] - samples[2]);
    expect(samples[6]).toBeLessThan(80);
  });

  it('does not chatter between armed and pulling near the release threshold', () => {
    expect(pullState('pulling', 0, 83).phase).toBe('pulling');
    expect(pullState('armed', 0, 83).phase).toBe('armed');
    expect(pullState('armed', 0, 72).phase).toBe('armed');
    expect(pullState('armed', 0, 71).phase).toBe('pulling');
    expect(pullState('armed', 0, 3).phase).toBe('pulling');
  });
});

describe('pullLabel', () => {
  it('says what a release would do, and nothing while idle', () => {
    expect(pullLabel('idle')).toBe('');
    expect(pullLabel('tracking')).toBe('');
    expect(pullLabel('pulling')).toBe('Pull to refresh');
    expect(pullLabel('armed')).toBe('Release to refresh');
    expect(pullLabel('refreshing')).toBe('Refreshing…');
  });
});
