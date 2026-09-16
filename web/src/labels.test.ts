import { describe, expect, it } from 'vitest';
import { originalLabel } from './labels';

function element(dataset: Record<string, string>, ariaLabel?: string) {
  return { dataset, getAttribute: (name: string) => (name === 'aria-label' ? ariaLabel ?? null : null) };
}

describe('original labels', () => {
  it('prefers the label saved before external-link decoration', () => {
    expect(originalLabel(element({ aggrExternalLabel: 'Play Launch on YouTube' }, 'Play Launch on YouTube, opens in a new tab'))).toBe('Play Launch on YouTube');
  });

  it('falls back to the current aria-label, then to nothing', () => {
    expect(originalLabel(element({}, 'Play Launch on YouTube'))).toBe('Play Launch on YouTube');
    expect(originalLabel(element({}))).toBe('');
    expect(originalLabel(element({ aggrExternalLabel: '' }, ''))).toBe('');
  });
});
