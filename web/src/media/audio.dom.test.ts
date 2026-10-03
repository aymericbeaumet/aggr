import { afterEach, describe, expect, it, vi } from 'vitest';
import { enhanceAudio } from './audio';

// The audio card as `item.html` writes it, with a known archived length.
const CARD = `<div class="native-audio has-artwork" data-audio-player data-audio-component>
    <div class="audio-cover"><img class="audio-artwork" src="cover.jpg" alt="" loading="lazy" decoding="async"></div>
    <div class="audio-panel">
      <div class="audio-heading">
        <p class="audio-show">The Show</p>
        <p class="audio-episode">Episode</p>
        <p class="audio-status" data-audio-status role="status" aria-live="polite"></p>
      </div>
      <audio controls preload="none" data-duration-seconds="600" src="episode.mp3" aria-label="Episode">
        <a href="episode.mp3" target="_blank" rel="noopener noreferrer">Open audio</a>
      </audio>
      <div class="audio-controls" data-audio-controls hidden>
        <div class="audio-transport">
          <button class="audio-skip" type="button" data-audio-skip="-30" aria-label="Back 30 seconds" title="Back 30 seconds" disabled><span aria-hidden="true">30</span></button><button class="audio-skip" type="button" data-audio-skip="-15" data-audio-back aria-label="Back 15 seconds" title="Back 15 seconds" disabled><span aria-hidden="true">15</span></button>
          <button class="audio-toggle" type="button" data-audio-toggle aria-label="Play" title="Play"></button>
          <button class="audio-skip" type="button" data-audio-skip="15" aria-label="Forward 15 seconds" title="Forward 15 seconds" disabled><span aria-hidden="true">15</span></button><button class="audio-skip" type="button" data-audio-skip="30" data-audio-forward aria-label="Forward 30 seconds" title="Forward 30 seconds" disabled><span aria-hidden="true">30</span></button>
        </div>
        <div class="audio-timeline">
          <input type="range" min="0" max="1000" value="0" step="1" data-audio-seek aria-label="Playback position" disabled>
          <div class="audio-times"><span data-audio-elapsed>0:00</span><span class="audio-ends-at" data-audio-ends-at></span><span data-audio-duration>--:--</span></div>
        </div>
      </div>
      <div class="audio-tools" data-audio-tools hidden>
        <label class="audio-speed">Speed <select data-audio-speed aria-label="Playback speed">
          <option value="0.5">0.5×</option><option value="0.75">0.75×</option><option value="1" selected>1×</option><option value="1.25">1.25×</option><option value="1.5">1.5×</option><option value="1.75">1.75×</option><option value="2">2×</option><option value="2.5">2.5×</option><option value="3">3×</option>
        </select></label>
        <div class="audio-volume">
          <button type="button" data-audio-mute aria-label="Mute" title="Mute" aria-pressed="false"></button>
          <input data-audio-volume type="range" min="0" max="1" step="0.05" value="1" aria-label="Volume">
          <span data-device-volume hidden title="Use the volume buttons on your device">Device volume</span>
        </div>
      </div>
    </div>
  </div>`;

type Fake = {
  card: HTMLElement;
  audio: HTMLAudioElement;
  play: ReturnType<typeof vi.fn>;
  pause: ReturnType<typeof vi.fn>;
  load: ReturnType<typeof vi.fn>;
  metadata(duration: number): void;
};

/**
 * jsdom has no media pipeline: playback, timing and volume are stood in for on the element so
 * the bindings can be exercised.
 */
function fixture(): Fake {
  const card = document.createElement('div');
  card.innerHTML = CARD;
  document.body.append(card);
  const root = card.firstElementChild as HTMLElement;
  const audio = root.querySelector('audio') as HTMLAudioElement;
  let paused = true;
  let duration = NaN;
  let volume = 1;
  const state = { currentTime: 0, playbackRate: 1, muted: false, ended: false, error: null as null | object };
  Object.defineProperties(audio, {
    paused: { get: () => paused, configurable: true },
    duration: { get: () => duration, configurable: true },
    currentTime: { get: () => state.currentTime, set: (value: number) => (state.currentTime = value), configurable: true },
    playbackRate: { get: () => state.playbackRate, set: (value: number) => (state.playbackRate = value), configurable: true },
    volume: { get: () => volume, set: (value: number) => (volume = value), configurable: true },
    muted: { get: () => state.muted, set: (value: boolean) => (state.muted = value), configurable: true },
    ended: { get: () => state.ended, configurable: true },
    error: { get: () => state.error, configurable: true },
  });
  const play = vi.fn(() => {
    paused = false;
    audio.dispatchEvent(new Event('play'));
    audio.dispatchEvent(new Event('playing'));
    return Promise.resolve();
  });
  const pause = vi.fn(() => {
    if (paused) return;
    paused = true;
    audio.dispatchEvent(new Event('pause'));
  });
  const load = vi.fn();
  Object.assign(audio, { play, pause, load });
  return {
    card: root,
    audio,
    play,
    pause,
    load,
    metadata(seconds) {
      duration = seconds;
      audio.dispatchEvent(new Event('loadedmetadata'));
    },
  };
}

const flush = () => new Promise<void>((resolve) => queueMicrotask(resolve));

afterEach(() => {
  document.body.innerHTML = '';
});

describe('enhanceAudio', () => {
  it('reveals the controls and shows the archived length before anything loads', async () => {
    const { card } = fixture();
    enhanceAudio(card, new AbortController().signal);
    expect(card.classList.contains('is-enhanced')).toBe(true);
    expect(card.querySelector<HTMLElement>('[data-audio-controls]')?.hidden).toBe(false);
    expect(card.querySelector<HTMLElement>('[data-audio-tools]')?.hidden).toBe(false);
    await flush();
    expect(card.dataset.audioState).toBe('idle');
    expect(card.querySelector('[data-audio-duration]')?.textContent).toBe('10:00');
    expect(card.querySelector('[data-audio-elapsed]')?.textContent).toBe('0:00');
    expect(card.querySelector('[data-audio-ends-at]')?.textContent).toBe('10:00 remaining');
    expect(card.querySelector<HTMLInputElement>('[data-audio-seek]')?.disabled).toBe(true);
    for (const button of card.querySelectorAll<HTMLButtonElement>('[data-audio-skip]')) expect(button.disabled).toBe(true);
    expect(card.querySelector('[data-audio-toggle]')?.getAttribute('aria-label')).toBe('Play');
  });

  it('plays, pauses, skips and seeks through the bound controls once the file is seekable', async () => {
    const { card, audio, play, pause, metadata } = fixture();
    enhanceAudio(card, new AbortController().signal);
    metadata(600);
    const seek = card.querySelector<HTMLInputElement>('[data-audio-seek]');
    expect(seek?.disabled).toBe(false);
    for (const button of card.querySelectorAll<HTMLButtonElement>('[data-audio-skip]')) expect(button.disabled).toBe(false);

    card.querySelector<HTMLButtonElement>('[data-audio-toggle]')?.click();
    expect(play).toHaveBeenCalledTimes(1);
    await flush();
    expect(card.dataset.audioState).toBe('playing');
    expect(card.querySelector('[data-audio-toggle]')?.getAttribute('aria-label')).toBe('Pause');
    expect(card.querySelector('[data-audio-status]')?.textContent).toBe('Playing');
    expect(card.querySelector('[data-audio-ends-at]')?.textContent).toMatch(/^Ends at \d\d:\d\d$/);

    card.querySelector<HTMLButtonElement>('[data-audio-skip="30"]')?.click();
    expect(audio.currentTime).toBe(30);
    expect(card.querySelector('[data-audio-elapsed]')?.textContent).toBe('0:30');
    expect(seek?.value).toBe('50');
    card.querySelector<HTMLButtonElement>('[data-audio-skip="-15"]')?.click();
    expect(audio.currentTime).toBe(15);

    if (seek) {
      seek.value = '500';
      seek.dispatchEvent(new Event('change'));
    }
    expect(audio.currentTime).toBe(300);
    expect(seek?.getAttribute('aria-valuetext')).toBe('5:00 of 10:00');

    card.querySelector<HTMLButtonElement>('[data-audio-toggle]')?.click();
    expect(pause).toHaveBeenCalled();
    expect(card.dataset.audioState).toBe('paused');
    expect(card.querySelector('[data-audio-ends-at]')?.textContent).toBe('5:00 remaining');
  });

  it('follows the speed select into the estimate, and mutes to zero and back', async () => {
    const { card, audio, metadata } = fixture();
    enhanceAudio(card, new AbortController().signal);
    metadata(600);
    const speed = card.querySelector<HTMLSelectElement>('[data-audio-speed]');
    if (speed) {
      speed.value = '2';
      speed.dispatchEvent(new Event('change'));
    }
    expect(audio.playbackRate).toBe(2);
    expect(card.querySelector('[data-audio-ends-at]')?.textContent).toBe('5:00 remaining');

    const mute = card.querySelector<HTMLButtonElement>('[data-audio-mute]');
    const volume = card.querySelector<HTMLInputElement>('[data-audio-volume]');
    if (volume) {
      volume.value = '0.6';
      volume.dispatchEvent(new Event('input'));
    }
    expect(audio.volume).toBeCloseTo(0.6);
    mute?.click();
    expect(audio.muted).toBe(true);
    expect(audio.volume).toBe(0);
    expect(volume?.value).toBe('0');
    expect(mute?.getAttribute('aria-label')).toBe('Unmute');
    expect(mute?.getAttribute('aria-pressed')).toBe('true');
    mute?.click();
    expect(audio.muted).toBe(false);
    expect(audio.volume).toBeCloseTo(0.6);
    expect(mute?.getAttribute('aria-label')).toBe('Mute');
  });

  it('brings the native controls back when playback fails', async () => {
    const { card, audio, play } = fixture();
    play.mockImplementation(() => Promise.reject(new Error('blocked')));
    enhanceAudio(card, new AbortController().signal);
    card.querySelector<HTMLButtonElement>('[data-audio-toggle]')?.click();
    await flush();
    await flush();
    expect(card.dataset.audioState).toBe('error');
    expect(audio.hasAttribute('data-native-visible')).toBe(true);
    expect(card.querySelector('[data-audio-status]')?.textContent).toBe('Audio unavailable. Try playing again.');
  });

  it('stops and releases the recording when the page is left', () => {
    const { card, audio, pause, load, metadata } = fixture();
    const controller = new AbortController();
    enhanceAudio(card, controller.signal);
    metadata(600);
    card.querySelector<HTMLButtonElement>('[data-audio-toggle]')?.click();
    controller.abort();
    expect(pause).toHaveBeenCalled();
    expect(audio.hasAttribute('src')).toBe(false);
    expect(load).toHaveBeenCalled();
    // Disposed: the controls no longer reach the player.
    card.querySelector<HTMLButtonElement>('[data-audio-skip="30"]')?.click();
    expect(audio.currentTime).toBe(0);
  });

  it('leaves a card without its controls alone', () => {
    const card = document.createElement('div');
    card.innerHTML = '<audio src="x.mp3"></audio>';
    enhanceAudio(card, new AbortController().signal);
    expect(card.classList.contains('is-enhanced')).toBe(false);
  });
});
