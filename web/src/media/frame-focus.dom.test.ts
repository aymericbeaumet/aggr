import { afterEach, describe, expect, it } from 'vitest';
import { installFrameFocus, resetFrameFocus } from './frame-focus';

function page(): { main: HTMLElement; frame: HTMLIFrameElement } {
  const main = document.createElement('main');
  main.id = 'content';
  main.tabIndex = -1;
  const frame = document.createElement('iframe');
  document.body.append(main, frame);
  return { main, frame };
}

const nextFrame = () => new Promise((resolve) => requestAnimationFrame(resolve));
const later = () => new Promise((resolve) => setTimeout(resolve, 60));

afterEach(() => {
  resetFrameFocus();
  document.body.replaceChildren();
});

describe('installFrameFocus', () => {
  it('hands the keyboard back when a frame becomes active after the blur', async () => {
    const { main, frame } = page();
    frame.className = 'interactive-viewer';
    installFrameFocus();
    window.dispatchEvent(new Event('blur'));
    await nextFrame();
    expect(document.activeElement).not.toBe(main);
    frame.focus();
    await later();
    expect(document.activeElement).toBe(main);
  });

  it('leaves a twitch frame and a fullscreen frame focused', async () => {
    const { frame } = page();
    frame.dataset.keys = 'player';
    installFrameFocus();
    frame.focus();
    window.dispatchEvent(new Event('blur'));
    await nextFrame();
    await later();
    expect(document.activeElement).toBe(frame);

    frame.removeAttribute('data-keys');
    Object.defineProperty(document, 'fullscreenElement', { configurable: true, get: () => frame });
    try {
      frame.focus();
      window.dispatchEvent(new Event('blur'));
      await nextFrame();
      await later();
      expect(document.activeElement).toBe(frame);
    } finally {
      delete (document as { fullscreenElement?: Element }).fullscreenElement;
    }
  });
});
