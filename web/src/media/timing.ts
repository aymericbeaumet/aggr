/**
 * The arithmetic behind a player's readouts: usable lengths, clock faces, and when a recording
 * playing at some speed will finish. Pure, so every player and its tests share one answer.
 */

export type Phase = 'idle' | 'loading' | 'playing' | 'paused' | 'ended' | 'error';

/** Where a player is: what it is doing, how far in, how long the recording is, how fast it runs. */
export type TimingState = {
  phase: Phase;
  /** Seconds into the recording; NaN until the player has said. */
  position: number;
  /** The recording's length in seconds; undefined for live or unknown media. */
  duration?: number;
  /** The playback rate; NaN until the player has said. */
  speed: number;
};

export type Estimate = {
  /** Seconds of listening left at the current speed. */
  remaining?: number;
  /** The wall-clock finish (epoch milliseconds), only while the recording actually plays. */
  endsAt?: number;
};

/** A usable, finite length in seconds, or nothing. */
export function duration(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) && value > 0 && value <= Number.MAX_SAFE_INTEGER
    ? value
    : undefined;
}

/** `m:ss`, or `h:mm:ss` past the hour; a dash for anything that is not a time. */
export function clock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return '—';
  const whole = Math.floor(seconds);
  const hours = Math.floor(whole / 3600);
  const minutes = Math.floor(whole / 60) % 60;
  const pad = (value: number) => String(value).padStart(2, '0');
  return hours ? `${hours}:${pad(minutes)}:${pad(whole % 60)}` : `${minutes}:${pad(whole % 60)}`;
}

/**
 * When the recording finishes, given where it is and how fast it is playing. A finish time is
 * only promised while it plays: paused, buffering, live or broken players have none.
 */
export function endsAt(state: TimingState, now = Date.now()): Estimate {
  const length = duration(state.duration);
  const speed = duration(state.speed);
  if (!length || !speed || !Number.isFinite(state.position)) return {};
  const remaining = Math.max(0, length - Math.max(0, state.position)) / speed;
  const end = now + remaining * 1000;
  return {
    remaining,
    endsAt:
      state.phase === 'playing' && remaining > 0 && Number.isFinite(end) && Math.abs(end) <= 8.64e15
        ? end
        : undefined,
  };
}

/** `HH:MM` on the reader's own clock. */
export function wallClock(timestamp: number): string {
  const date = new Date(timestamp);
  if (!Number.isFinite(date.getTime())) return '';
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
}

/**
 * The "Ends at HH:MM" line the template reserves beside a player (`[data-media-timing]`, its
 * next sibling). A player without one, such as a facade inside the prose, reports into nothing.
 */
export function timingHost(player: Element): (state: TimingState) => void {
  const sibling = player.nextElementSibling;
  const host = sibling?.matches('[data-media-timing]') ? sibling : null;
  return (state) => {
    if (!host) return;
    const { endsAt: end } = endsAt(state);
    const text = end === undefined ? '' : `Ends at ${wallClock(end)}`;
    if (host.textContent !== text) host.textContent = text;
  };
}

/**
 * Follow a native `<video>` for its timing line. The archived length stands in until the file
 * says its own; leaving the page stops the download as well as the playback.
 */
export function followNativeVideo(video: HTMLVideoElement, signal: AbortSignal): void {
  const frame = video.closest('.native-video');
  if (!frame) return;
  const report = timingHost(frame);
  const archived = duration(Number(video.dataset.durationSeconds));
  let phase: Phase = video.paused ? 'paused' : video.readyState >= 3 ? 'playing' : 'loading';
  const emit = () =>
    report({
      phase,
      duration: Number.isNaN(video.duration) ? archived : duration(video.duration),
      position: video.currentTime,
      speed: video.playbackRate,
    });
  const setPhase = (next: Phase) => {
    phase = next;
    emit();
  };
  for (const name of ['loadedmetadata', 'durationchange', 'timeupdate', 'ratechange']) {
    video.addEventListener(name, emit, { signal });
  }
  video.addEventListener('play', () => setPhase('loading'), { signal });
  video.addEventListener('playing', () => setPhase('playing'), { signal });
  video.addEventListener('waiting', () => setPhase(video.paused ? 'paused' : 'loading'), { signal });
  video.addEventListener('pause', () => setPhase(video.ended ? 'ended' : 'paused'), { signal });
  video.addEventListener('ended', () => setPhase('ended'), { signal });
  video.addEventListener('error', () => setPhase('error'), { signal });
  signal.addEventListener(
    'abort',
    () => {
      video.pause();
      video.removeAttribute('src');
      video.load();
    },
    { once: true },
  );
  emit();
}
