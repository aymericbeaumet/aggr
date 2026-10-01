import { afterEach, describe, expect, it, vi } from 'vitest';
import { foldProgress, installReadingHeader } from './readingHeader';

describe('reading header without scroll timelines', () => {
  afterEach(() => {
    vi.restoreAllMocks();
    document.body.innerHTML = '';
    delete document.body.dataset.kind;
  });

  it('folds over ten rem of scroll', () => {
    expect(foldProgress(0, 16)).toBe(0);
    expect(foldProgress(80, 16)).toBe(0.5);
    expect(foldProgress(400, 16)).toBe(1);
    expect(foldProgress(-10, 16)).toBe(0);
  });

  it('drives the fold and the bar from the scroll position, and lets go with the page', () => {
    document.body.dataset.kind = 'item';
    document.body.innerHTML = '<header class="itemhead"><h1><span class="itemhead-title">T</span></h1><span class="itemhead-progress"></span></header>';
    const header = document.querySelector('.itemhead') as HTMLElement;
    vi.spyOn(CSS, 'supports').mockReturnValue(false);
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    Object.defineProperty(window, 'scrollY', { value: 80, configurable: true });
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((callback) => {
      callback(0);
      return 1;
    });
    const controller = new AbortController();
    installReadingHeader(controller.signal);
    expect(header.style.getPropertyValue('--header-progress')).toBe('0.5');
    Object.defineProperty(window, 'scrollY', { value: 400, configurable: true });
    window.dispatchEvent(new Event('scroll'));
    expect(header.style.getPropertyValue('--header-progress')).toBe('1');
    controller.abort();
    expect(header.style.getPropertyValue('--header-progress')).toBe('');
  });
});
