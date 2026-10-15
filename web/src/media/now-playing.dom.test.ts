import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import NowPlaying from '../app/NowPlaying.svelte';
import { enhanceAudio } from './audio';
import { dismiss, resetPlayback } from './now-playing';

const CARD = `<div class="native-audio has-artwork" data-audio-player data-audio-component>
    <div class="audio-cover"><img class="audio-artwork" src="cover.jpg" alt=""></div>
    <div class="audio-panel">
      <div class="audio-heading"><p class="audio-show">The Show</p><p class="audio-episode">Episode</p></div>
      <audio controls preload="none" data-duration-seconds="600" src="episode.mp3" aria-label="Episode"></audio>
      <div class="audio-controls" data-audio-controls hidden>
        <button type="button" data-audio-toggle aria-label="Play"></button>
        <input type="range" min="0" max="1000" value="0" data-audio-seek>
        <select data-audio-speed><option value="1" selected>1×</option><option value="1.25">1.25×</option><option value="1.5">1.5×</option><option value="2">2×</option></select>
      </div>
      <div class="audio-tools" data-audio-tools hidden></div>
    </div>
  </div>`;

type Started = {
  card: HTMLElement;
  audio: HTMLAudioElement;
  pause: ReturnType<typeof vi.fn>;
  /** Load the length and press play. Call after any setup that must precede promotion. */
  begin(): void;
};

function arm(src = 'episode.mp3'): Started {
  const holder = document.createElement('div');
  holder.innerHTML = CARD.replaceAll('episode.mp3', src);
  document.body.append(holder);
  const card = holder.firstElementChild as HTMLElement;
  const audio = card.querySelector('audio') as HTMLAudioElement;
  let paused = true;
  let length = NaN;
  const state = { currentTime: 0, playbackRate: 1, muted: false, volume: 1 };
  Object.defineProperties(audio, {
    paused: { get: () => paused, configurable: true },
    duration: { get: () => length, configurable: true },
    currentTime: { get: () => state.currentTime, set: (value: number) => (state.currentTime = value), configurable: true },
    playbackRate: { get: () => state.playbackRate, set: (value: number) => (state.playbackRate = value), configurable: true },
    volume: { get: () => state.volume, set: (value: number) => (state.volume = value), configurable: true },
    muted: { get: () => state.muted, set: (value: boolean) => (state.muted = value), configurable: true },
    ended: { get: () => false, configurable: true },
    error: { get: () => null, configurable: true },
  });
  const pause = vi.fn(() => {
    if (paused) return;
    paused = true;
    audio.dispatchEvent(new Event('pause'));
  });
  Object.assign(audio, {
    play: vi.fn(() => {
      paused = false;
      audio.dispatchEvent(new Event('play'));
      audio.dispatchEvent(new Event('playing'));
      return Promise.resolve();
    }),
    pause,
    load: vi.fn(),
  });
  enhanceAudio(card, new AbortController().signal);
  return {
    card,
    audio,
    pause,
    begin() {
      length = 600;
      audio.dispatchEvent(new Event('loadedmetadata'));
      card.querySelector<HTMLButtonElement>('[data-audio-toggle]')?.click();
    },
  };
}

function start(src = 'episode.mp3'): Started {
  const started = arm(src);
  started.begin();
  return started;
}

function onScreen(card: HTMLElement): void {
  card.getBoundingClientRect = () =>
    ({
      x: 0,
      y: 0,
      top: 8,
      left: 8,
      right: 200,
      bottom: 180,
      width: 192,
      height: 172,
      toJSON() {},
    }) as DOMRect;
}

let view: ReturnType<typeof mount> | null = null;

function dock(): HTMLElement {
  const target = document.createElement('div');
  document.body.append(target);
  view = mount(NowPlaying, { target });
  flushSync();
  return target;
}

const settle = () => new Promise<void>((resolve) => queueMicrotask(resolve));

type SessionHandler = (details: MediaSessionActionDetails) => void;

type FakeSession = {
  metadata: { title?: string; artist?: string; artwork?: { src: string }[] } | null;
  playbackState: MediaSessionPlaybackState;
  handlers: Map<string, SessionHandler | null>;
  positions: MediaPositionState[];
};

let restoreMedia: (() => void) | null = null;

function installMediaSession(): FakeSession {
  const fake: FakeSession = { metadata: null, playbackState: 'none', handlers: new Map(), positions: [] };
  const session = {
    get metadata() {
      return fake.metadata;
    },
    set metadata(value: FakeSession['metadata']) {
      fake.metadata = value;
    },
    get playbackState() {
      return fake.playbackState;
    },
    set playbackState(value: MediaSessionPlaybackState) {
      fake.playbackState = value;
    },
    setActionHandler(action: string, handler: SessionHandler | null) {
      fake.handlers.set(action, handler);
    },
    setPositionState(state?: MediaPositionState) {
      if (state) fake.positions.push({ ...state });
    },
  };
  const previous = Object.getOwnPropertyDescriptor(navigator, 'mediaSession');
  const PreviousMetadata = window.MediaMetadata;
  Object.defineProperty(navigator, 'mediaSession', { configurable: true, value: session });
  window.MediaMetadata = class {
    title: string;
    artist: string;
    artwork: { src: string; sizes?: string }[];
    constructor(init: MediaMetadataInit) {
      this.title = init.title ?? '';
      this.artist = init.artist ?? '';
      this.artwork = init.artwork ? [...init.artwork] : [];
    }
  } as unknown as typeof MediaMetadata;
  restoreMedia = () => {
    if (previous) Object.defineProperty(navigator, 'mediaSession', previous);
    else Reflect.deleteProperty(navigator, 'mediaSession');
    window.MediaMetadata = PreviousMetadata;
    restoreMedia = null;
  };
  return fake;
}

function press(fake: FakeSession, action: string, details: Partial<MediaSessionActionDetails> = {}): void {
  fake.handlers.get(action)?.({ action, ...details } as MediaSessionActionDetails);
}

afterEach(() => {
  if (view) unmount(view);
  view = null;
  resetPlayback();
  restoreMedia?.();
  sessionStorage.clear();
  document.body.innerHTML = '';
  document.documentElement.removeAttribute('data-now-playing');
  document.documentElement.style.removeProperty('--now-playing-offset');
});

describe('now playing', () => {
  it('advances the elapsed time without a timeupdate from the element', async () => {
    const target = dock();
    const { audio } = start();
    flushSync();
    audio.currentTime = 4.2;
    await new Promise((resolve) => setTimeout(resolve, 400));
    flushSync();
    expect(target.querySelector('.now-playing-time')?.textContent).toBe('0:04');
  });

  it('renders nothing until a recording is playing', () => {
    const target = dock();
    expect(target.querySelector('.now-playing')).toBeNull();
    expect(document.documentElement.hasAttribute('data-now-playing')).toBe(false);
  });

  it('follows a recording after its page is left and stops when dismissed', async () => {
    const target = dock();
    const { card, audio, pause } = start();
    flushSync();
    const bar = target.querySelector('.now-playing');
    expect(bar).not.toBeNull();
    expect(bar?.querySelector('.now-playing-show')?.textContent).toBe('The Show');
    expect(bar?.querySelector('.now-playing-title')?.textContent).toBe('Episode');
    expect(bar?.querySelector('a')?.getAttribute('href')).toBe(location.href.split('#')[0]);
    expect(document.documentElement.hasAttribute('data-now-playing')).toBe(true);
    expect(audio.paused).toBe(false);

    card.remove();
    await settle();
    flushSync();
    expect(target.querySelector('.now-playing')).not.toBeNull();
    expect(audio.paused).toBe(false);

    target.querySelector<HTMLButtonElement>('.now-playing-close')?.click();
    flushSync();
    expect(target.querySelector('.now-playing')).toBeNull();
    expect(pause).toHaveBeenCalled();
    expect(audio.hasAttribute('src')).toBe(false);
    expect(document.documentElement.hasAttribute('data-now-playing')).toBe(false);
  });

  it('hides while the episode is on screen and returns when the card leaves', async () => {
    const target = dock();
    const { card, audio, begin } = arm();
    onScreen(card);
    begin();
    flushSync();
    expect(target.querySelector('.now-playing')).toBeNull();
    expect(audio.paused).toBe(false);
    expect(document.documentElement.hasAttribute('data-now-playing')).toBe(false);

    card.remove();
    await settle();
    flushSync();
    expect(target.querySelector('.now-playing')).not.toBeNull();
    expect(document.documentElement.hasAttribute('data-now-playing')).toBe(true);
  });

  it('seeks, skips and cycles speed from the bar, in step with the card', () => {
    const target = dock();
    const { card, audio } = start();
    flushSync();
    target.querySelector<HTMLButtonElement>('[aria-label="Forward 30 seconds"]')?.click();
    expect(audio.currentTime).toBe(30);
    target.querySelector<HTMLButtonElement>('[aria-label="Back 15 seconds"]')?.click();
    expect(audio.currentTime).toBe(15);
    const seek = target.querySelector<HTMLInputElement>('.now-playing-seek');
    if (seek) {
      seek.value = '500';
      seek.dispatchEvent(new Event('input', { bubbles: true }));
      seek.dispatchEvent(new Event('change', { bubbles: true }));
    }
    flushSync();
    expect(audio.currentTime).toBe(300);
    expect(target.querySelector('.now-playing-time')?.textContent).toBe('5:00');

    target.querySelector<HTMLButtonElement>('.now-playing-speed')?.click();
    flushSync();
    expect(audio.playbackRate).toBe(1.25);
    expect(target.querySelector('.now-playing-speed')?.textContent).toBe('1.25×');
    expect(card.querySelector('select')?.value).toBe('1.25');
  });

  it('stops when the site is left', () => {
    const { audio, pause } = start();
    window.dispatchEvent(new Event('pagehide'));
    expect(pause).toHaveBeenCalled();
    expect(audio.paused).toBe(true);
    expect(audio.hasAttribute('src')).toBe(false);
  });

  it('remembers the speed and the position for the next time that episode plays', () => {
    const first = start();
    const speed = first.card.querySelector<HTMLSelectElement>('select');
    if (speed) {
      speed.value = '1.5';
      speed.dispatchEvent(new Event('change'));
    }
    first.audio.currentTime = 120;
    first.card.querySelector<HTMLButtonElement>('[data-audio-toggle]')?.click();
    dismiss();

    const again = start();
    expect(again.audio.playbackRate).toBe(1.5);
    expect(again.audio.currentTime).toBe(120);

    const chosen = arm('other.mp3');
    const select = chosen.card.querySelector<HTMLSelectElement>('select');
    if (select) {
      select.value = '2';
      select.dispatchEvent(new Event('change'));
    }
    chosen.begin();
    // Chosen on the card before play, so the previous episode's speed stays put.
    expect(chosen.audio.playbackRate).toBe(2);
  });

  it('follows system media keys and keeps the lock-screen position moving', async () => {
    const target = dock();
    const session = installMediaSession();
    const { audio, pause } = start();
    flushSync();
    expect(session.handlers.get('play')).toEqual(expect.any(Function));
    expect(session.handlers.get('pause')).toEqual(expect.any(Function));
    expect(session.handlers.get('previoustrack')).toEqual(expect.any(Function));
    expect(session.handlers.get('nexttrack')).toEqual(expect.any(Function));
    expect(session.metadata?.title).toBe('Episode');
    expect(session.metadata?.artist).toBe('The Show');
    expect(session.metadata?.artwork?.[0]?.src).toBe(new URL('cover.jpg', location.href).href);
    expect(session.playbackState).toBe('playing');
    expect(session.positions.at(-1)).toMatchObject({ position: 0, duration: 600, playbackRate: 1 });

    press(session, 'play');
    expect(audio.paused).toBe(false);
    press(session, 'pause');
    expect(audio.paused).toBe(true);
    expect(pause).toHaveBeenCalledTimes(1);
    press(session, 'pause');
    expect(pause).toHaveBeenCalledTimes(1);
    press(session, 'play');
    expect(audio.paused).toBe(false);

    press(session, 'seekforward', { seekOffset: 10 });
    expect(audio.currentTime).toBe(10);
    press(session, 'nexttrack');
    expect(audio.currentTime).toBe(40);
    press(session, 'seekbackward');
    expect(audio.currentTime).toBe(25);
    // Previous is the same 15-second skip, so a session action and a key in one turn do not stack.
    press(session, 'previoustrack');
    expect(audio.currentTime).toBe(25);
    press(session, 'seekto', { seekTime: 900 });
    expect(audio.currentTime).toBe(600);

    sessionStorage.clear();
    audio.currentTime = 4.2;
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(session.positions.at(-1)?.position).toBeGreaterThanOrEqual(4);
    expect(sessionStorage.length).toBe(0);
    expect(target.querySelector('.now-playing-time')?.textContent).toBe('0:04');

    press(session, 'stop');
    expect(pause).toHaveBeenCalled();
    expect(session.handlers.get('play')).toBeNull();
    expect(session.playbackState).toBe('none');
  });

  it('treats a hardware key and the matching session action as one press', () => {
    const session = installMediaSession();
    const { audio } = start();
    audio.currentTime = 0;
    press(session, 'nexttrack');
    const next = new KeyboardEvent('keydown', { key: 'MediaTrackNext', cancelable: true });
    window.dispatchEvent(next);
    expect(next.defaultPrevented).toBe(true);
    expect(audio.currentTime).toBe(30);

    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'MediaPlayPause', cancelable: true }));
    expect(audio.paused).toBe(true);
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'MediaPlay', cancelable: true }));
    expect(audio.paused).toBe(false);
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'MediaTrackPrevious', cancelable: true }));
    expect(audio.currentTime).toBe(15);
    const space = new KeyboardEvent('keydown', { key: ' ', cancelable: true });
    window.dispatchEvent(space);
    expect(space.defaultPrevented).toBe(false);
    expect(audio.paused).toBe(false);
  });

  it('mutes and sets the volume of parked audio from the bar', () => {
    const target = dock();
    const { audio } = start();
    flushSync();
    const slider = target.querySelector<HTMLInputElement>('.now-playing-level');
    expect(slider).not.toBeNull();
    if (slider) {
      slider.value = '0.5';
      slider.dispatchEvent(new Event('input', { bubbles: true }));
      slider.dispatchEvent(new Event('change', { bubbles: true }));
    }
    flushSync();
    expect(audio.volume).toBeCloseTo(0.5);
    expect(slider?.getAttribute('aria-valuetext')).toBe('50%');
    target.querySelector<HTMLButtonElement>('.now-playing-mute')?.click();
    flushSync();
    expect(audio.muted).toBe(true);
    expect(audio.volume).toBe(0);
    expect(target.querySelector('.now-playing-mute')?.getAttribute('aria-pressed')).toBe('true');
    expect(target.querySelector('.now-playing-mute')?.getAttribute('aria-label')).toBe('Unmute');
  });

  it('hides the dock volume when the element ignores it', () => {
    const target = dock();
    const { audio } = start();
    flushSync();
    Object.defineProperty(audio, 'volume', { configurable: true, get: () => 1, set() {} });
    const slider = target.querySelector<HTMLInputElement>('.now-playing-level');
    if (slider) {
      slider.value = '0.4';
      slider.dispatchEvent(new Event('input', { bubbles: true }));
    }
    flushSync();
    expect(target.querySelector('.now-playing-level')).toBeNull();
    expect(target.querySelector('.now-playing-mute')).not.toBeNull();
  });
});
