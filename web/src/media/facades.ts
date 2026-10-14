import { SPEEDS, type Playback, type PlaybackState } from './audio';
import { installFrameFocus } from './frame-focus';
import { claimVisual, connectVisual, visualPlaying, type Episode } from './now-playing';
import { duration, endsAt, timingHost, type Phase, type TimingState } from './timing';

/**
 * Provider players are facades: nothing reaches YouTube, Vimeo or Twitch until the reader
 * activates the poster, and activation plays at once by asking the player directly rather than
 * trusting an `autoplay` parameter. Timing comes back over postMessage, so no provider SDK is
 * ever downloaded; a message counts only when it comes from the frame's own window and origin.
 */

export type Provider = 'youtube' | 'vimeo';

/** One widget id for listening and commands. A second id makes YouTube ignore the transport. */
const WIDGET_ID = 'aggr';

const VIMEO_EVENTS = [
  'play',
  'playing',
  'pause',
  'ended',
  'timeupdate',
  'playbackratechange',
  'bufferstart',
  'bufferend',
  'seeking',
  'seeked',
  'durationchange',
  'error',
];

type Data = Record<string, unknown>;

const asRecord = (value: unknown): Data | undefined =>
  value && typeof value === 'object' && !Array.isArray(value) ? (value as Data) : undefined;

function messageData(value: unknown): Data | undefined {
  if (typeof value === 'string') {
    if (value.length > 128_000) return undefined;
    try {
      return asRecord(JSON.parse(value));
    } catch {
      return undefined;
    }
  }
  return asRecord(value);
}

const youtubePhase = (value: unknown): Phase | undefined =>
  value === 1
    ? 'playing'
    : value === 2
      ? 'paused'
      : value === 3
        ? 'loading'
        : value === 0
          ? 'ended'
          : value === -1 || value === 5
            ? 'idle'
            : undefined;

const finite = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value);

/**
 * Follow a provider player's state over postMessage. Returns the subscription to send once the
 * frame has loaded; the player's own ready message sends it again if it came first.
 */
export function followProvider(
  frame: HTMLIFrameElement,
  provider: Provider,
  origin: string,
  report: (state: TimingState) => void,
  known: number,
  signal: AbortSignal,
): () => void {
  let state: TimingState = { phase: 'idle', position: NaN, duration: duration(known), speed: NaN };
  let live = false;
  let recorded = Boolean(state.duration);
  let subscribed = false;
  let stale = false;
  let vimeoPlayback: Phase = 'idle';
  let watchdog: ReturnType<typeof setTimeout> | undefined;
  signal.addEventListener('abort', () => clearTimeout(watchdog), { once: true });

  const post = (message: unknown) => frame.contentWindow?.postMessage(message, origin);
  function emit(): void {
    clearTimeout(watchdog);
    report({ ...state });
    // A player that stops reporting is buffering, not still playing.
    if (state.phase === 'playing' && state.duration) {
      watchdog = setTimeout(() => {
        stale = true;
        state = { ...state, phase: 'loading' };
        report({ ...state });
      }, 5000);
    }
  }

  function subscribe(): void {
    if (provider === 'youtube') {
      post(JSON.stringify({ event: 'listening', id: WIDGET_ID, channel: 'widget' }));
      for (const event of ['onStateChange', 'onPlaybackRateChange', 'onError']) {
        post(JSON.stringify({ event: 'command', func: 'addEventListener', args: [event], id: WIDGET_ID, channel: 'widget' }));
      }
    } else {
      for (const event of VIMEO_EVENTS) post({ method: 'addEventListener', value: event });
      for (const method of ['getDuration', 'getCurrentTime', 'getPlaybackRate', 'getPaused']) post({ method });
    }
    subscribed = true;
  }

  function youtube(data: Data): boolean {
    if (data.event === 'readyToListen' || data.event === 'onReady') {
      subscribe();
      return false;
    }
    if (data.event === 'initialDelivery' || data.event === 'infoDelivery') {
      const info = asRecord(data.info);
      if (!info) return false;
      const video = asRecord(info.videoData);
      if (typeof video?.isLive === 'boolean') {
        live = video.isLive;
        recorded = !live;
      }
      if (live) state.duration = undefined;
      else if (recorded) {
        if (info.duration !== undefined) state.duration = duration(info.duration);
        const progress = asRecord(info.progressState);
        if (state.duration === undefined && progress?.duration !== undefined) state.duration = duration(progress.duration);
      }
      if (finite(info.currentTime) && info.currentTime >= 0) {
        state.position = info.currentTime;
        if (stale) state.phase = 'playing';
        stale = false;
      }
      if (info.playbackRate !== undefined) state.speed = duration(info.playbackRate) ?? NaN;
      const phase = youtubePhase(info.playerState);
      if (phase) {
        state.phase = phase;
        stale = false;
      }
      return true;
    }
    if (data.event === 'onStateChange') {
      state.phase = youtubePhase(data.info) ?? 'idle';
      stale = false;
      return true;
    }
    if (data.event === 'onPlaybackRateChange') {
      state.speed = duration(data.info) ?? NaN;
      return true;
    }
    if (data.event === 'onError') {
      state.phase = 'error';
      stale = false;
      return true;
    }
    return false;
  }

  function vimeo(data: Data): boolean {
    if (data.event === 'ready') {
      if (!subscribed) subscribe();
      return false;
    }
    const info = asRecord(data.data);
    if (data.method === 'getDuration') state.duration = duration(data.value);
    else if (data.method === 'getCurrentTime' && finite(data.value) && data.value >= 0) state.position = data.value;
    else if (data.method === 'getPlaybackRate') state.speed = duration(data.value) ?? NaN;
    else if (data.method === 'getPaused') {
      if (data.value === true) {
        state.phase = vimeoPlayback = 'paused';
        stale = false;
      }
    } else if (typeof data.event === 'string' && VIMEO_EVENTS.includes(data.event)) {
      if (info?.duration !== undefined) state.duration = duration(info.duration);
      if (finite(info?.seconds) && info.seconds >= 0) {
        state.position = info.seconds;
        if (data.event === 'timeupdate' && stale) {
          state.phase = vimeoPlayback;
          stale = false;
        }
      }
      if (!['timeupdate', 'playbackratechange', 'durationchange'].includes(data.event)) stale = false;
      if (info?.playbackRate !== undefined) state.speed = duration(info.playbackRate) ?? NaN;
      if (data.event === 'playing') state.phase = vimeoPlayback = 'playing';
      else if (['play', 'bufferstart', 'seeking'].includes(data.event)) state.phase = 'loading';
      else if (['seeked', 'bufferend'].includes(data.event)) state.phase = vimeoPlayback;
      else if (data.event === 'pause') state.phase = vimeoPlayback = 'paused';
      else if (data.event === 'ended') state.phase = vimeoPlayback = 'ended';
      else if (data.event === 'error') state.phase = vimeoPlayback = 'error';
    } else return false;
    return true;
  }

  window.addEventListener(
    'message',
    (event) => {
      if (!frame.contentWindow || event.source !== frame.contentWindow || event.origin !== origin) return;
      const data = messageData(event.data);
      if (!data) return;
      if (provider === 'youtube' ? youtube(data) : vimeo(data)) emit();
    },
    { signal },
  );

  return subscribe;
}

const LOCAL_HOSTS = ['localhost', '127.0.0.1', '[::1]'];

/** Identity of an embed, before the autoplay and origin parameters activation adds. */
function embedIdentity(raw: string): string {
  try {
    const url = new URL(raw, location.href);
    for (const key of ['autoplay', 'enablejsapi', 'origin', 'parent']) url.searchParams.delete(key);
    url.hash = '';
    return url.href;
  } catch {
    return '';
  }
}

function episodeOf(player: HTMLElement, preview: HTMLAnchorElement, src: string): Episode {
  const raw =
    preview.dataset.aggrExternalLabel ||
    preview.getAttribute('aria-label')?.replace(/, (?:opens in a new tab|external site)$/, '') ||
    '';
  const named = raw.match(/^Play (.+) on (YouTube|Vimeo|Twitch)$/);
  const provider = player.dataset.videoProvider ?? '';
  const fallback = provider === 'youtube' ? 'YouTube' : provider === 'vimeo' ? 'Vimeo' : provider === 'twitch' ? 'Twitch' : '';
  const image = preview.querySelector('img');
  return {
    src,
    title: named?.[1] || raw.replace(/^Play\s+/, '') || 'Video',
    show: named?.[2] || fallback,
    artwork: image instanceof HTMLImageElement && image.getAttribute('src') ? image.src : '',
    page: typeof location === 'undefined' ? '' : location.href.split('#')[0],
  };
}

function toPlayback(state: TimingState): PlaybackState {
  const total = duration(state.duration);
  const position = Number.isFinite(state.position) ? Math.max(0, state.position) : 0;
  const timing = { ...state, position };
  return {
    ...timing,
    progress: total ? Math.min(1, position / total) : 0,
    seekable: Boolean(total),
    volume: 1,
    muted: false,
    volumeAdjustable: false,
    ...endsAt(timing),
  };
}

/**
 * Drive a YouTube or Vimeo frame over postMessage. Twitch has no such API here: the frame is
 * only moved, and its own controls stay the ones that play and seek it. The listener lives as
 * long as the playback does, which is the session, not the page.
 */
function createEmbedPlayback(
  frame: HTMLIFrameElement,
  provider: Provider | 'twitch',
  origin: string,
  known: number | undefined,
  update: (state: PlaybackState) => void,
): {
  playback: Playback;
  subscribe: () => void;
  signal: AbortSignal;
  resumable: () => boolean;
  assume: (phase: Phase) => void;
} {
  const listeners = new AbortController();
  let state: TimingState = { phase: 'idle', position: NaN, duration: duration(known), speed: NaN };
  const emit = (next: TimingState) => {
    state = next;
    update(toPlayback(state));
  };
  const post = (message: unknown) => {
    try {
      frame.contentWindow?.postMessage(message, origin);
    } catch {
      /* the frame may already be gone */
    }
  };
  const subscribe =
    provider === 'youtube' || provider === 'vimeo'
      ? followProvider(
          frame,
          provider,
          origin,
          (timing) => {
            // Unstarted and cued arrive after the reader has already asked the player to start.
            const phase = timing.phase === 'idle' && state.phase !== 'idle' ? state.phase : timing.phase;
            emit({ ...timing, phase });
          },
          known ?? 0,
          listeners.signal,
        )
      : () => {};

  const command = (func: string, args: unknown[] = []) =>
    post(JSON.stringify({ event: 'command', func, args, id: WIDGET_ID, channel: 'widget' }));
  const vimeo = (method: string, value?: number) => post(value === undefined ? { method } : { method, value });

  function seek(fraction: number): void {
    const total = duration(state.duration);
    if (!Number.isFinite(fraction) || !total) return;
    const seconds = Math.max(0, Math.min(1, fraction)) * total;
    if (provider === 'youtube') command('seekTo', [seconds, true]);
    else if (provider === 'vimeo') vimeo('setCurrentTime', seconds);
    else return;
    emit({ ...state, position: seconds });
  }

  const playback: Playback = {
    toggle() {
      if (provider !== 'youtube' && provider !== 'vimeo') return;
      const pausing = state.phase === 'playing' || state.phase === 'loading';
      if (provider === 'youtube') command(pausing ? 'pauseVideo' : 'playVideo');
      else vimeo(pausing ? 'pause' : 'play');
      emit({ ...state, phase: pausing ? 'paused' : 'loading' });
    },
    seek,
    skip(seconds) {
      const total = duration(state.duration);
      const position = Number.isFinite(state.position) ? state.position : 0;
      if (total && Number.isFinite(seconds)) seek((position + seconds) / total);
    },
    speed(value) {
      if (!SPEEDS.includes(value) || (provider !== 'youtube' && provider !== 'vimeo')) return;
      if (provider === 'youtube') command('setPlaybackRate', [value]);
      else vimeo('setPlaybackRate', value);
      emit({ ...state, speed: value });
    },
    volume() {},
    mute() {},
    dispose() {
      listeners.abort();
    },
  };

  return {
    playback,
    subscribe,
    signal: listeners.signal,
    /** A paused or finished recording must not be started again by a later load. */
    resumable: () => state.phase === 'loading' || state.phase === 'playing',
    assume(phase) {
      emit({ ...state, phase });
    },
  };
}

/** Turn a poster into the player it stands for, on the reader's activation and never before. */
export function enhanceVideoFacade(preview: HTMLAnchorElement, signal: AbortSignal): void {
  const player = preview.closest('.video-player');
  if (!(player instanceof HTMLElement) || player.dataset.videoBound === 'true') return;
  const provider = player.dataset.videoProvider;
  // Twitch refuses to embed outside a secure context.
  if (provider === 'twitch' && location.protocol !== 'https:' && !LOCAL_HOSTS.includes(location.hostname)) return;
  player.dataset.videoBound = 'true';
  const title =
    preview.dataset.aggrExternalLabel ||
    preview.getAttribute('aria-label')?.replace(/, (?:opens in a new tab|external site)$/, '') ||
    'Video';
  const identity = embedIdentity(preview.dataset.videoEmbed || '');
  const episode = episodeOf(player, preview, identity);
  const report = (state: PlaybackState) => {
    if (!signal.aborted) timingHost(player)(state);
  };
  // Coming back to this recording reuses the frame already playing. The frame stays in its seat,
  // so it is not a child of this card.
  if (identity && claimVisual(player, episode, report, signal)) {
    preview.hidden = true;
    player.classList.add('is-loaded');
    player.classList.remove('is-loading');
    player.dataset.videoMounted = 'true';
  }

  function activate(event: MouseEvent | KeyboardEvent): void {
    if (event instanceof MouseEvent && (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)) return;
    if (event instanceof KeyboardEvent && event.key !== ' ' && event.key !== 'Enter') return;
    event.preventDefault();
    if (!(player instanceof HTMLElement) || player.dataset.videoMounted === 'true' || !identity) return;
    if (visualPlaying(identity)) return;
    player.dataset.videoMounted = 'true';
    player.dataset.playingSrc = identity;

    let url: URL;
    try {
      url = new URL(identity);
    } catch {
      return;
    }
    if (preview.hasAttribute('data-video-parent')) url.searchParams.set('parent', location.hostname);
    url.searchParams.set('autoplay', provider === 'twitch' ? 'true' : '1');
    if (provider === 'youtube') {
      url.searchParams.set('enablejsapi', '1');
      url.searchParams.set('origin', location.origin);
    }

    const frame = document.createElement('iframe');
    frame.src = url.href;
    frame.title = title;
    frame.loading = 'eager';
    frame.allow = 'autoplay; encrypted-media; fullscreen; picture-in-picture';
    frame.allowFullscreen = true;
    frame.referrerPolicy = 'strict-origin-when-cross-origin';
    // YouTube delivers commands only to the widget id registered by `listening`.
    if (provider === 'youtube') frame.id = WIDGET_ID;
    frame.setAttribute('sandbox', 'allow-scripts allow-same-origin allow-presentation');

    const bridge: { push: (state: PlaybackState) => void } = { push() {} };
    const opened = createEmbedPlayback(
      frame,
      provider === 'youtube' || provider === 'vimeo' ? provider : 'twitch',
      url.origin,
      duration(Number(player.dataset.durationSeconds)),
      (state) => bridge.push(state),
    );
    bridge.push = connectVisual(frame, episode, opened.playback, report, signal, {
      controls: provider === 'youtube' || provider === 'vimeo',
    }).push;

    player.classList.add('is-loading');
    player.setAttribute('aria-busy', 'true');
    // Twitch draws its own keys, including after this frame leaves the player for the picture.
    if (provider === 'twitch') frame.dataset.keys = 'player';
    // Hand the keyboard back after the click, and again if the provider focuses the frame late.
    installFrameFocus();
    // Append before the loading state promotes, so the card is where the picture returns.
    player.appendChild(frame);
    opened.assume('loading');
    frame.addEventListener(
      'load',
      () => {
        player.classList.add('is-loaded');
        player.removeAttribute('aria-busy');
        preview.hidden = true;
        // Every load, including one the provider starts itself, has to be told we are listening.
        // A frame that reloaded otherwise stops reporting time and ignores the bar.
        opened.subscribe();
        if (provider === 'twitch') {
          opened.assume('playing');
          return;
        }
        if (!opened.resumable()) return;
        // `autoplay` alone leaves YouTube and Vimeo showing their own play button when the
        // browser declines; the activating click already authorized this, so ask the player too.
        const target = frame.contentWindow;
        if (!target) return;
        if (provider === 'youtube') {
          target.postMessage(
            JSON.stringify({ event: 'command', func: 'playVideo', args: [], id: WIDGET_ID, channel: 'widget' }),
            url.origin,
          );
        } else if (provider === 'vimeo') target.postMessage({ method: 'play' }, url.origin);
      },
      { signal: opened.signal },
    );
    // Twitch refuses to render below 400×300. A narrow column gets a player laid out at that
    // size and scaled down to fit, rather than a player that widens the article or a refusal.
    if (provider === 'twitch') {
      const fit = (width: number, height: number) => {
        const scale = width && width < 400 ? width / 400 : 1;
        frame.style.width = scale < 1 ? '400px' : '100%';
        frame.style.height = scale < 1 ? `${Math.max(300, Math.ceil((400 * height) / width))}px` : '100%';
        frame.style.transform = scale < 1 ? `scale(${scale})` : '';
        frame.style.transformOrigin = 'top left';
      };
      const box = player.getBoundingClientRect();
      fit(box.width, box.height);
      if ('ResizeObserver' in window) {
        const observer = new ResizeObserver((entries) => {
          for (const entry of entries) fit(entry.contentRect.width, entry.contentRect.height);
        });
        observer.observe(player);
        signal.addEventListener('abort', () => observer.disconnect(), { once: true });
      }
    }
  }

  preview.setAttribute('role', 'button');
  preview.addEventListener('click', activate, { signal });
  preview.addEventListener('keydown', activate, { signal });
}
