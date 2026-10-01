import { duration, timingHost, type Phase, type TimingState } from './timing';

/**
 * Provider players are facades: nothing reaches YouTube, Vimeo or Twitch until the reader
 * activates the poster, and activation plays at once by asking the player directly rather than
 * trusting an `autoplay` parameter. Timing comes back over postMessage, so no provider SDK is
 * ever downloaded; a message counts only when it comes from the frame's own window and origin.
 */

export type Provider = 'youtube' | 'vimeo';

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
      post(JSON.stringify({ event: 'listening', id: 'aggr-timing', channel: 'widget' }));
      for (const event of ['onStateChange', 'onPlaybackRateChange', 'onError']) {
        post(JSON.stringify({ event: 'command', func: 'addEventListener', args: [event], id: 'aggr-timing', channel: 'widget' }));
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
      else if (recorded && info.duration !== undefined) state.duration = duration(info.duration);
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

  function activate(event: MouseEvent | KeyboardEvent): void {
    if (event instanceof MouseEvent && (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)) return;
    if (event instanceof KeyboardEvent && event.key !== ' ' && event.key !== 'Enter') return;
    event.preventDefault();
    if (!(player instanceof HTMLElement) || player.dataset.videoMounted === 'true') return;
    player.dataset.videoMounted = 'true';

    let url: URL;
    try {
      url = new URL(preview.dataset.videoEmbed || '');
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
    frame.setAttribute('sandbox', 'allow-scripts allow-same-origin allow-presentation');

    const report = timingHost(player);
    const subscribe =
      provider === 'youtube' || provider === 'vimeo'
        ? followProvider(frame, provider, url.origin, report, Number(player.dataset.durationSeconds), signal)
        : null;

    player.classList.add('is-loading');
    player.setAttribute('aria-busy', 'true');
    frame.addEventListener(
      'load',
      () => {
        player.classList.add('is-loaded');
        player.removeAttribute('aria-busy');
        preview.hidden = true;
        subscribe?.();
        // `autoplay` alone leaves YouTube and Vimeo showing their own play button when the
        // browser declines; the activating click already authorized this, so ask the player too.
        const target = frame.contentWindow;
        if (!target) return;
        if (provider === 'youtube') {
          target.postMessage(JSON.stringify({ event: 'listening', id: 'aggr-play', channel: 'widget' }), url.origin);
          target.postMessage(
            JSON.stringify({ event: 'command', func: 'playVideo', args: [], id: 'aggr-play', channel: 'widget' }),
            url.origin,
          );
        } else if (provider === 'vimeo') target.postMessage({ method: 'play' }, url.origin);
      },
      { once: true, signal },
    );
    player.appendChild(frame);
    frame.focus({ preventScroll: true });
    signal.addEventListener(
      'abort',
      () => {
        frame.removeAttribute('src');
        frame.remove();
      },
      { once: true },
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
