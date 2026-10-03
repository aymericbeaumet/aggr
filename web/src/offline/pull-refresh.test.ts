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
    expect(pullState('tracking', 0, 6)).toEqual({ phase: 'pulling', distance: 3 });
    expect(pullState('pulling', 0, 40)).toEqual({ phase: 'pulling', distance: 22 });
    expect(pullState('pulling', 0, 84)).toEqual({ phase: 'armed', distance: 46 });
    expect(pullState('armed', 0, 200)).toEqual({ phase: 'armed', distance: 72 });
    expect(pullState('armed', 0, 60)).toEqual({ phase: 'pulling', distance: 33 });
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
