import { afterEach, describe, expect, it, vi } from 'vitest';
import { announce } from './announce';

afterEach(() => vi.unstubAllGlobals());

describe('live announcements', () => {
  it('clears the announcer, then writes the message on the next frame so repeats are heard', () => {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => frames.push(callback));
    const announcer = { textContent: 'Navigated to Home' };
    const root = { querySelector: (selector: string) => selector === '#aggr-announcer' ? announcer : null } as unknown as Document;
    announce('Navigated to Home', root);
    expect(announcer.textContent).toBe('');
    expect(frames).toHaveLength(1);
    frames[0](0);
    expect(announcer.textContent).toBe('Navigated to Home');
  });

  it('does nothing on a page without the announcer', () => {
    const requestAnimationFrame = vi.fn();
    vi.stubGlobal('requestAnimationFrame', requestAnimationFrame);
    expect(() => announce('Offline', { querySelector: () => null } as unknown as Document)).not.toThrow();
    expect(requestAnimationFrame).not.toHaveBeenCalled();
  });
});
