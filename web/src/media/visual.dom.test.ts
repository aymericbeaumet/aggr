import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import NowPlaying from '../app/NowPlaying.svelte';
import { enhanceAudio } from './audio';
import { enhanceVideoFacade } from './facades';
import { resetFrameFocus } from './frame-focus';
import { dismiss, resetPlayback } from './now-playing';
import { watchNativeVideo } from './visual';

const EMBED = 'https://www.youtube-nocookie.com/embed/abc';

function rect(node: HTMLElement): void {
  node.getBoundingClientRect = () =>
    ({
      x: 0,
      y: 0,
      top: 8,
      left: 8,
      right: 320,
      bottom: 188,
      width: 312,
      height: 180,
      toJSON() {},
    }) as DOMRect;
}

function videoCard(src = 'clip.mp4'): { frame: HTMLElement; video: HTMLVideoElement } {
  const frame = document.createElement('div');
  frame.className = 'native-video media-frame';
  const video = document.createElement('video');
  video.controls = true;
  video.setAttribute('src', src);
  video.setAttribute('aria-label', 'Clip');
  video.dataset.durationSeconds = '90';
  video.pause = vi.fn();
  video.load = vi.fn();
  frame.append(video);
  document.body.append(frame);
  return { frame, video };
}

/** A native play, without going through the element the way a click on its controls would. */
function play(video: HTMLVideoElement): void {
  let paused = false;
  Object.defineProperty(video, 'paused', { configurable: true, get: () => paused });
  video.pause = vi.fn(() => {
    paused = true;
  });
  video.load = vi.fn();
  video.dispatchEvent(new Event('play'));
  video.dispatchEvent(new Event('playing'));
}

function player(provider: string, embed = EMBED): { player: HTMLElement; preview: HTMLAnchorElement } {
  const root = document.createElement('div');
  root.className = 'video-player';
  root.dataset.videoProvider = provider;
  root.dataset.durationSeconds = '90';
  root.innerHTML = `<a class="video-preview" data-video-embed="${embed}" href="https://example.test/watch" aria-label="Play Clip on ${provider === 'youtube' ? 'YouTube' : provider === 'vimeo' ? 'Vimeo' : 'Twitch'}">Poster</a>`;
  document.body.append(root);
  const preview = root.querySelector('a');
  if (!(preview instanceof HTMLAnchorElement)) throw new Error('missing preview');
  return { player: root, preview };
}

let view: ReturnType<typeof mount> | null = null;

const seat = () => document.querySelector<HTMLElement>('#now-playing-picture');
const fit = () => seat()?.querySelector('.now-playing-fit');

function dock(): HTMLElement {
  const target = document.createElement('div');
  document.body.append(target);
  view = mount(NowPlaying, { target });
  flushSync();
  return target;
}

afterEach(() => {
  if (view) unmount(view);
  view = null;
  resetPlayback();
  resetFrameFocus();
  document.body.replaceChildren();
  document.documentElement.removeAttribute('data-now-playing');
  document.documentElement.style.removeProperty('--now-playing-offset');
});

describe('visual now playing', () => {
  it('keeps a started video when its page is left and releases one that never played', () => {
    const started = videoCard();
    const page = new AbortController();
    watchNativeVideo(started.video, page.signal);
    play(started.video);
    page.abort();
    expect(started.video.paused).toBe(false);
    expect(started.video.hasAttribute('src')).toBe(true);
    expect(fit()?.contains(started.video)).toBe(true);

    const idle = videoCard('other.mp4');
    const left = new AbortController();
    watchNativeVideo(idle.video, left.signal);
    left.abort();
    expect(idle.video.hasAttribute('src')).toBe(false);
    expect(idle.frame.contains(idle.video)).toBe(true);
    expect(document.getElementById('now-playing-park')?.contains(idle.video) ?? false).toBe(false);
  });

  it('puts the picture in the bar off screen and back in the article on screen', () => {
    const target = dock();
    const { frame, video } = videoCard();
    rect(frame);
    watchNativeVideo(video, new AbortController().signal);
    play(video);
    flushSync();
    expect(target.querySelector('.now-playing')).toBeNull();
    expect(fit()?.contains(video)).toBe(true);
    expect(seat()?.dataset.at).toBe('card');

    frame.getBoundingClientRect = () =>
      ({ x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0, toJSON() {} }) as DOMRect;
    window.dispatchEvent(new Event('scroll'));
    flushSync();
    expect(target.querySelector('.now-playing')?.hasAttribute('data-visual')).toBe(true);
    expect(target.querySelector('.now-playing-volume')).toBeNull();
    expect(fit()?.contains(video)).toBe(true);
    expect(seat()?.dataset.at).toBe('dock');
    expect(video).toBe(fit()?.querySelector('video'));

    rect(frame);
    window.dispatchEvent(new Event('scroll'));
    flushSync();
    expect(target.querySelector('.now-playing')).toBeNull();
    expect(fit()?.contains(video)).toBe(true);
    expect(seat()?.dataset.at).toBe('card');
    expect(video.hasAttribute('src')).toBe(true);
  });

  it('returns a parked video to the same recording and drops the duplicate', () => {
    const first = videoCard();
    const left = new AbortController();
    watchNativeVideo(first.video, left.signal);
    play(first.video);
    left.abort();
    first.frame.remove();

    const second = videoCard();
    rect(second.frame);
    watchNativeVideo(second.video, new AbortController().signal);
    expect(second.video.isConnected).toBe(false);
    expect(fit()?.contains(first.video)).toBe(true);
    expect(seat()?.dataset.at).toBe('card');
    expect(first.video.hasAttribute('src')).toBe(true);
  });

  it('stops the video when another recording starts', () => {
    const first = videoCard();
    watchNativeVideo(first.video, new AbortController().signal);
    play(first.video);

    const second = videoCard('other.mp4');
    watchNativeVideo(second.video, new AbortController().signal);
    play(second.video);
    expect(first.video.hasAttribute('src')).toBe(false);
    expect(first.frame.contains(first.video)).toBe(true);
    expect(second.video.paused).toBe(false);
    expect(fit()?.contains(second.video)).toBe(true);
  });

  it('leaves a provider frame in the article while that player is on screen', () => {
    const target = dock();
    const { player: root, preview } = player('youtube');
    rect(root);
    enhanceVideoFacade(preview, new AbortController().signal);
    preview.click();
    flushSync();
    expect(root.dataset.playingSrc).toContain('/embed/abc');
    expect(fit()?.querySelector('iframe')).not.toBeNull();
    expect(root.querySelector('iframe')).toBeNull();
    expect(seat()?.dataset.at).toBe('card');
    expect(target.querySelector('.now-playing')).toBeNull();
  });

  it('keeps keyboard focus on the page when a provider frame starts', async () => {
    const main = document.createElement('main');
    main.id = 'content';
    main.tabIndex = -1;
    document.body.append(main);
    const { player: root, preview } = player('youtube');
    main.append(root);
    preview.focus();
    enhanceVideoFacade(preview, new AbortController().signal);
    preview.click();
    const frame = document.querySelector('iframe');
    expect(frame).toBeInstanceOf(HTMLIFrameElement);
    if (!(frame instanceof HTMLIFrameElement)) return;
    frame.focus();
    // jsdom records the iframe as focused without delivering the focus event to the element.
    // A browser click is a window blur with the frame already active, which is handed back
    // on the next frame.
    if (document.activeElement === frame) {
      window.dispatchEvent(new Event('blur'));
      await new Promise((resolve) => requestAnimationFrame(resolve));
    }
    expect(document.activeElement).toBe(main);
  });

  it('returns keyboard focus before a focused provider frame is removed', () => {
    const main = document.createElement('main');
    main.id = 'content';
    main.tabIndex = -1;
    document.body.append(main);
    const { player: root, preview } = player('twitch', 'https://player.twitch.tv/?video=99');
    main.append(root);
    enhanceVideoFacade(preview, new AbortController().signal);
    preview.click();
    const frame = document.querySelector('iframe');
    expect(frame).toBeInstanceOf(HTMLIFrameElement);
    if (!(frame instanceof HTMLIFrameElement)) return;
    frame.focus();
    expect(document.activeElement).toBe(frame);
    dismiss();
    expect(frame.isConnected).toBe(false);
    expect(document.activeElement).toBe(main);
  });

  it('keeps a started provider frame and restores it into the same embed', () => {
    const first = player('youtube');
    const left = new AbortController();
    enhanceVideoFacade(first.preview, left.signal);
    first.preview.click();
    const frame = document.querySelector('iframe');
    expect(frame?.getAttribute('src')).toContain('autoplay=1');
    expect(frame?.getAttribute('src')).toContain('enablejsapi=1');
    left.abort();
    expect(frame?.isConnected).toBe(true);
    expect(frame?.getAttribute('src')).toContain('/embed/abc');
    expect(fit()?.contains(frame as Node)).toBe(true);
    first.player.remove();

    const again = player('youtube');
    rect(again.player);
    enhanceVideoFacade(again.preview, new AbortController().signal);
    expect(fit()?.contains(frame as Node)).toBe(true);
    expect(again.preview.hidden).toBe(true);
    expect(document.querySelectorAll('iframe')).toHaveLength(1);
  });

  it('releases a provider poster that was never activated', () => {
    const { player: root, preview } = player('youtube');
    const page = new AbortController();
    enhanceVideoFacade(preview, page.signal);
    page.abort();
    expect(root.querySelector('iframe')).toBeNull();
    expect(document.getElementById('now-playing-park')).toBeNull();
  });

  it('moves a Twitch frame without offering seek or speed', () => {
    const handlers = new Map<string, unknown>();
    let playbackState = 'playing';
    const previous = Object.getOwnPropertyDescriptor(navigator, 'mediaSession');
    Object.defineProperty(navigator, 'mediaSession', {
      configurable: true,
      value: {
        metadata: { title: 'leftover' },
        get playbackState() {
          return playbackState;
        },
        set playbackState(value: string) {
          playbackState = value;
        },
        setActionHandler(action: string, handler: unknown) {
          handlers.set(action, handler);
        },
        setPositionState() {},
      },
    });
    try {
      const target = dock();
      const { preview } = player('twitch', 'https://player.twitch.tv/?video=99');
      enhanceVideoFacade(preview, new AbortController().signal);
      preview.click();
      flushSync();
      const bar = target.querySelector('.now-playing');
      const frame = document.querySelector('iframe');
      expect(fit()?.contains(frame as Node)).toBe(true);
      expect(seat()?.dataset.at).toBe('dock');
      expect(bar?.querySelector<HTMLButtonElement>('.audio-toggle')?.disabled).toBe(true);
      expect(bar?.querySelector<HTMLInputElement>('.now-playing-seek')?.disabled).toBe(true);
      expect(bar?.querySelector<HTMLButtonElement>('.now-playing-speed')?.disabled).toBe(true);
      expect(bar?.querySelector('.now-playing-volume')).toBeNull();
      expect(frame?.getAttribute('sandbox')).toBe('allow-scripts allow-same-origin allow-presentation');
      expect(handlers.get('play')).toBeNull();
      expect(handlers.get('nexttrack')).toBeNull();
      expect(playbackState).toBe('none');
    } finally {
      if (previous) Object.defineProperty(navigator, 'mediaSession', previous);
      else Reflect.deleteProperty(navigator, 'mediaSession');
    }
  });

  it('stops the provider frame when a podcast starts', () => {
    const { player: root, preview } = player('vimeo', 'https://player.vimeo.com/video/77');
    enhanceVideoFacade(preview, new AbortController().signal);
    preview.click();
    const frame = document.querySelector('iframe');
    expect(frame).not.toBeNull();
    expect(root.querySelector('iframe')).toBeNull();

    const card = document.createElement('div');
    card.className = 'native-audio';
    card.dataset.audioComponent = '';
    card.innerHTML = `<div class="audio-heading"><p class="audio-episode">Episode</p></div><audio src="episode.mp3"></audio><div data-audio-controls hidden><button type="button" data-audio-toggle></button></div><div data-audio-tools hidden></div>`;
    document.body.append(card);
    const audio = card.querySelector('audio');
    if (!(audio instanceof HTMLAudioElement)) throw new Error('missing audio');
    let paused = true;
    Object.defineProperty(audio, 'paused', { configurable: true, get: () => paused });
    audio.play = vi.fn(() => {
      paused = false;
      audio.dispatchEvent(new Event('play'));
      audio.dispatchEvent(new Event('playing'));
      return Promise.resolve();
    });
    audio.pause = vi.fn();
    audio.load = vi.fn();
    enhanceAudio(card, new AbortController().signal);
    card.querySelector<HTMLButtonElement>('[data-audio-toggle]')?.click();
    expect(frame?.isConnected).toBe(false);
    expect(preview.hidden).toBe(false);
    expect(audio.paused).toBe(false);
  });

  it('drops the provider frame when the bar is closed', () => {
    const { player: root, preview } = player('youtube');
    enhanceVideoFacade(preview, new AbortController().signal);
    preview.click();
    expect(fit()?.querySelector('iframe')).not.toBeNull();
    dismiss();
    expect(document.querySelector('iframe')).toBeNull();
    expect(preview.hidden).toBe(false);
    expect(seat()).toBeNull();
  });
});
