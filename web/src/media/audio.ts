import { clock, duration, endsAt, wallClock, type Phase, type TimingState } from './timing';

/**
 * The podcast player: a native `<audio>` driven from the controls the template already holds
 * (`data-audio-*`), hidden until enhancement succeeds. Nothing is rendered here; a failure
 * leaves the reader with the browser's own controls rather than nothing.
 */

export const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2, 2.5, 3];

export type PlaybackState = TimingState & {
  progress: number;
  seekable: boolean;
  volume: number;
  muted: boolean;
  volumeAdjustable: boolean;
  remaining?: number;
  endsAt?: number;
};

export type Playback = {
  toggle(): void;
  seek(fraction: number): void;
  skip(seconds: number): void;
  speed(value: number): void;
  volume(value: number): void;
  mute(): void;
  dispose(): void;
};

/** Drive a native `<audio>` element and report its state back to whoever renders it. */
export function createPlayback(
  audio: HTMLAudioElement,
  update: (state: PlaybackState) => void,
  known: { duration?: number } = {},
): Playback {
  const listeners = new AbortController();
  const signal = listeners.signal;
  let phase: Phase = audio.paused ? 'idle' : 'playing';
  let attempt = 0;
  let disposed = false;
  let volumeAdjustable = true;
  let previousVolume = audio.volume > 0 ? audio.volume : 1;

  const seekable = () => duration(audio.duration);
  const length = () => (audio.duration === Infinity ? undefined : (seekable() ?? duration(known.duration)));

  function emit(): void {
    if (disposed) return;
    const total = length();
    const position = Number.isFinite(audio.currentTime) ? Math.max(0, Math.min(audio.currentTime, total ?? Infinity)) : 0;
    if (audio.volume > 0) previousVolume = audio.volume;
    const state: TimingState = { phase, position, duration: total, speed: audio.playbackRate };
    update({
      ...state,
      progress: total ? position / total : 0,
      seekable: Boolean(seekable()),
      volume: audio.volume,
      muted: audio.muted || audio.volume === 0,
      volumeAdjustable,
      ...endsAt(state),
    });
  }
  const setPhase = (next: Phase) => {
    phase = next;
    emit();
  };

  for (const name of ['loadedmetadata', 'durationchange', 'timeupdate', 'ratechange', 'volumechange']) {
    audio.addEventListener(name, emit, { signal });
  }
  audio.addEventListener('play', () => setPhase('loading'), { signal });
  audio.addEventListener('playing', () => setPhase('playing'), { signal });
  audio.addEventListener('waiting', () => !audio.paused && setPhase('loading'), { signal });
  audio.addEventListener('pause', () => setPhase(audio.ended ? 'ended' : 'paused'), { signal });
  audio.addEventListener('ended', () => setPhase('ended'), { signal });
  audio.addEventListener('error', () => setPhase('error'), { signal });

  function seek(fraction: number): void {
    const total = seekable();
    if (disposed || !total || !Number.isFinite(fraction)) return;
    try {
      audio.currentTime = Math.max(0, Math.min(1, fraction)) * total;
      emit();
    } catch {
      /* not seekable yet */
    }
  }

  function volume(value: number): void {
    if (disposed || !Number.isFinite(value)) return;
    const next = Math.max(0, Math.min(1, value));
    try {
      audio.volume = next;
      // Some platforms hand volume entirely to the device; notice and say so.
      volumeAdjustable = Math.abs(audio.volume - next) < 0.001;
      if (next > 0 && volumeAdjustable) audio.muted = false;
    } catch {
      volumeAdjustable = false;
    }
    emit();
  }

  // The archived length is known before anything loads: show it rather than a placeholder.
  queueMicrotask(emit);

  return {
    toggle() {
      if (disposed) return;
      const token = ++attempt;
      if (!audio.paused || phase === 'loading') {
        audio.pause();
        setPhase('paused');
        return;
      }
      if (audio.error) audio.load();
      setPhase('loading');
      const fail = () => {
        if (!disposed && token === attempt) setPhase('error');
      };
      try {
        const started: unknown = audio.play();
        if (started instanceof Promise) started.catch(fail);
      } catch {
        fail();
      }
    },
    seek,
    skip(seconds) {
      const total = seekable();
      if (total && Number.isFinite(seconds)) seek((audio.currentTime + seconds) / total);
    },
    speed(value) {
      if (disposed || !SPEEDS.includes(value)) return;
      try {
        audio.playbackRate = value;
        emit();
      } catch {
        /* the browser may restrict playback rates */
      }
    },
    volume,
    /** Muting drops the visible level to zero; unmuting restores the level in use before. */
    mute() {
      if (disposed) return;
      try {
        if (audio.muted || audio.volume === 0) {
          audio.muted = false;
          volume(previousVolume);
          return;
        }
        previousVolume = audio.volume;
        audio.volume = 0;
        audio.muted = true;
      } catch {
        /* device-controlled volume */
      }
      emit();
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      attempt += 1;
      listeners.abort();
      audio.pause();
    },
  };
}

const STATUS: Partial<Record<Phase, string>> = {
  error: 'Audio unavailable. Try playing again.',
  loading: 'Loading audio…',
  playing: 'Playing',
  paused: 'Paused',
  ended: 'Finished',
};

const query = <T extends Element = HTMLElement>(root: ParentNode, selector: string): T | null =>
  root.querySelector<T>(selector);

/** Bind the card's controls to its `<audio>`; `signal` ends playback when the page is left. */
export function enhanceAudio(card: HTMLElement, signal: AbortSignal): void {
  const audio = card.querySelector('audio');
  const controls = query(card, '[data-audio-controls]');
  const tools = query(card, '[data-audio-tools]');
  if (!audio || !controls || !tools) return;

  const toggle = query(card, '[data-audio-toggle]');
  const seekBar = query<HTMLInputElement>(card, 'input[data-audio-seek]');
  const elapsed = query(card, '[data-audio-elapsed]');
  const total = query(card, '[data-audio-duration]');
  const status = query(card, '[data-audio-status]');
  const estimate = query(card, '[data-audio-ends-at]');
  const speed = query<HTMLSelectElement>(card, 'select[data-audio-speed]');
  const mute = query(card, '[data-audio-mute]');
  const volume = query<HTMLInputElement>(card, 'input[data-audio-volume]');
  const device = query(card, '[data-device-volume]');
  const skips = [...card.querySelectorAll<HTMLButtonElement>('button[data-audio-skip]')];
  let scrubbing = false;

  const player = createPlayback(
    audio,
    (state) => {
      card.dataset.audioState = state.phase;
      const active = state.phase === 'playing' || state.phase === 'loading';
      const label = active ? 'Pause' : state.phase === 'ended' ? 'Replay' : 'Play';
      if (toggle) {
        toggle.setAttribute('aria-label', label);
        toggle.title = label;
      }
      if (status) status.textContent = STATUS[state.phase] || '';
      if (elapsed) elapsed.textContent = clock(state.position);
      if (total) total.textContent = state.duration ? clock(state.duration) : '--:--';
      if (seekBar) {
        seekBar.disabled = !state.seekable;
        if (!scrubbing) seekBar.value = String(Math.round(state.progress * 1000));
        seekBar.setAttribute('aria-valuetext', `${clock(state.position)} of ${clock(state.duration ?? NaN)}`);
      }
      for (const button of skips) button.disabled = !state.seekable;
      // The finish time only means something while the recording plays; the total is beside it.
      if (estimate) {
        estimate.textContent =
          state.endsAt !== undefined
            ? `Ends at ${wallClock(state.endsAt)}`
            : state.remaining !== undefined
              ? `${clock(state.remaining)} remaining`
              : '';
      }
      if (mute) {
        const muteLabel = state.muted ? 'Unmute' : 'Mute';
        mute.setAttribute('aria-label', muteLabel);
        mute.setAttribute('aria-pressed', String(state.muted));
        mute.title = muteLabel;
      }
      if (volume && !volume.matches(':active')) {
        volume.hidden = !state.volumeAdjustable;
        volume.value = String(state.volume);
        volume.setAttribute('aria-valuetext', `${Math.round(state.volume * 100)}%`);
        if (device) device.hidden = state.volumeAdjustable;
      }
      if (speed) speed.value = String(state.speed);
      // Native controls come back if playback fails.
      audio.toggleAttribute('data-native-visible', state.phase === 'error');
    },
    { duration: Number(audio.dataset.durationSeconds) },
  );

  toggle?.addEventListener('click', () => player.toggle(), { signal });
  for (const button of skips) {
    button.addEventListener('click', () => player.skip(Number(button.dataset.audioSkip)), { signal });
  }
  seekBar?.addEventListener(
    'input',
    () => {
      scrubbing = true;
      player.seek(Number(seekBar.value) / 1000);
    },
    { signal },
  );
  seekBar?.addEventListener(
    'change',
    () => {
      scrubbing = false;
      player.seek(Number(seekBar.value) / 1000);
    },
    { signal },
  );
  seekBar?.addEventListener('blur', () => (scrubbing = false), { signal });
  speed?.addEventListener('change', () => player.speed(Number(speed.value)), { signal });
  mute?.addEventListener('click', () => player.mute(), { signal });
  volume?.addEventListener('input', () => player.volume(Number(volume.value)), { signal });

  controls.hidden = false;
  tools.hidden = false;
  card.classList.add('is-enhanced');
  window.addEventListener('pagehide', () => player.dispose(), { signal });
  signal.addEventListener(
    'abort',
    () => {
      player.dispose();
      audio.removeAttribute('src');
      audio.load();
    },
    { once: true },
  );
}
