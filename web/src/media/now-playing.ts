import { createPlayback, type Playback, type PlaybackState } from './audio';
import { duration, endsAt } from './timing';

/**
 * One recording at a time, kept outside the page so in-app navigation cannot take it with the
 * article. Audio moves into a clipped host once playback leaves idle — `display: none` can
 * suspend media — and stays there. A picture is seated once, at the size it started, and that
 * element stays there: the seat scales between the article and the bar. Reparenting or resizing a
 * provider frame reloads it, and the elapsed time and transport die with the reloaded player.
 * Until playback leaves idle the element stays put, where duration reporting and the browser
 * suite can still find it.
 */

export type Episode = {
  /** Absolute media URL; the identity of the recording. Embed URLs omit autoplay and origin. */
  src: string;
  title: string;
  show: string;
  artwork: string;
  /** Article address without a hash, so the bar can link back. */
  page: string;
};

export type Session = {
  episode: Episode;
  state: PlaybackState;
  /** The picture is the player. The bar shows it instead of only the artwork. */
  visual: boolean;
  /** Seek, skip and speed. A Twitch frame keeps the controls it draws itself. */
  controls: boolean;
};

type Slot = {
  /** Native audio or video. A provider frame has none. */
  audio: HTMLMediaElement | null;
  node: HTMLElement;
  visual: boolean;
  controls: boolean;
  episode: Episode;
  playback: Playback;
  render: (state: PlaybackState) => void;
  snapshot: PlaybackState | null;
  /** Promoted: the page must not dispose this element. */
  live: boolean;
  /** Remembered settings apply only after promotion has finished its own setup. */
  settled: boolean;
  resumed: boolean;
  savedAt: number;
  home: Element | null;
  anchor: ChildNode | null;
  /** Layout size of the picture when it was seated. The player keeps this size. */
  base: { width: number; height: number } | null;
  /** Where the picture is heading. Scroll tracking only follows the card. */
  at: 'card' | 'dock' | 'moving';
  motion: Animation | null;
  /** Identifies the glide currently in flight, so an older finish cannot retarget the seat. */
  motionToken: object | null;
  /** Provider position was sampled at this `performance.now()`. */
  sampleAt: number;
  /** Last whole second the clock published, so a quiet player still advances once a second. */
  clocked: number;
  unpicture?: () => void;
};

type Box = { left: number; top: number; width: number; height: number };

const HOST_ID = 'now-playing-audio';
const PARK_ID = 'now-playing-park';
const PICTURE_ID = 'now-playing-picture';
const RESUME_PREFIX = 'aggr:listen:';
const DOCK_SPEEDS = [1, 1.25, 1.5, 1.75, 2];
const GLIDE_MS = 280;

let live: Slot | null = null;
let rememberedRate: number | null = null;
let rememberedVolume: { volume: number; muted: boolean } | null = null;
let pagehideBound = false;
let clockHandle = 0;
/** The scale animation paired with `slot.motion`. One picture, so one flight. */
let fitMotion: Animation | null = null;
const listeners = new Set<(session: Session | null) => void>();

const noop = () => {};

function inertPlayback(): Playback {
  return { toggle: noop, seek: noop, skip: noop, speed: noop, volume: noop, mute: noop, dispose: noop };
}

function storage(): Storage | null {
  if (typeof document !== 'undefined' && document.prerendering) return null;
  try {
    return sessionStorage;
  } catch {
    return null;
  }
}

function releaseElement(audio: HTMLAudioElement): void {
  try {
    audio.pause();
  } catch {
    /* the element may already be gone */
  }
  audio.removeAttribute('src');
  try {
    audio.load();
  } catch {
    /* nothing loaded */
  }
}

function host(): HTMLElement {
  const existing = document.getElementById(HOST_ID);
  if (existing) return existing;
  const node = document.createElement('div');
  node.id = HOST_ID;
  node.setAttribute('aria-hidden', 'true');
  document.body.append(node);
  return node;
}

function picture(): { host: HTMLElement; fit: HTMLElement } {
  let host = document.getElementById(PICTURE_ID);
  if (!host) {
    host = document.createElement('div');
    host.id = PICTURE_ID;
    host.dataset.at = 'card';
    document.body.append(host);
  }
  const found = host.querySelector('.now-playing-fit');
  if (found instanceof HTMLElement) return { host, fit: found };
  const fit = document.createElement('div');
  fit.className = 'now-playing-fit';
  host.append(fit);
  return { host, fit };
}

function boxOf(rect: DOMRect): Box {
  return { left: rect.left, top: rect.top, width: rect.width, height: rect.height };
}

function usable(box: Box): boolean {
  return box.width >= 2 && box.height >= 2;
}

function near(a: Box, b: Box): boolean {
  return Math.abs(a.left - b.left) < 1 && Math.abs(a.top - b.top) < 1 && Math.abs(a.width - b.width) < 1 && Math.abs(a.height - b.height) < 1;
}

function motionOk(): boolean {
  if (typeof document !== 'undefined' && document.documentElement.dataset.motion === 'off') return false;
  try {
    return !window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
  } catch {
    return true;
  }
}

/** The border box minus the border, which is the box an absolutely positioned player fills. */
function contentBox(node: HTMLElement): Box {
  const rect = boxOf(node.getBoundingClientRect());
  const style = getComputedStyle(node);
  const left = parseFloat(style.borderLeftWidth) || 0;
  const right = parseFloat(style.borderRightWidth) || 0;
  const top = parseFloat(style.borderTopWidth) || 0;
  const bottom = parseFloat(style.borderBottomWidth) || 0;
  return {
    left: rect.left + left,
    top: rect.top + top,
    width: Math.max(0, rect.width - left - right),
    height: Math.max(0, rect.height - top - bottom),
  };
}

/** Scale the seated player into `box`, letterboxed, without changing its layout size. */
function fitTransform(base: { width: number; height: number }, box: Box): string {
  const sx = base.width ? box.width / base.width : 1;
  const sy = base.height ? box.height / base.height : 1;
  const scale = Math.min(sx, sy);
  const dx = (box.width - base.width * scale) / 2;
  const dy = (box.height - base.height * scale) / 2;
  return `translate(${dx}px, ${dy}px) scale(${scale})`;
}

function stopMotion(slot: Slot): void {
  slot.motion?.cancel();
  slot.motion = null;
  slot.motionToken = null;
  fitMotion?.cancel();
  fitMotion = null;
}

/** Draw the seat at `box`. The player keeps its seated size and is scaled to fit. */
function paint(slot: Slot, box: Box): void {
  const { host, fit } = picture();
  const base = slot.base ?? { width: box.width, height: box.height };
  slot.base = base;
  host.style.left = `${box.left}px`;
  host.style.top = `${box.top}px`;
  host.style.width = `${box.width}px`;
  host.style.height = `${box.height}px`;
  fit.style.width = `${base.width}px`;
  fit.style.height = `${base.height}px`;
  fit.style.transform = fitTransform(base, box);
}

function glide(slot: Slot, to: Box, at: 'card' | 'dock'): void {
  const { host, fit } = picture();
  stopMotion(slot);
  const from = boxOf(host.getBoundingClientRect());
  host.dataset.at = at;
  // Paint only when there is nothing to animate. Painting the destination first would flash
  // the seat to the end, then jump it back to the start of the flight.
  if (!motionOk() || !usable(from) || !usable(to) || near(from, to) || typeof host.animate !== 'function') {
    paint(slot, to);
    slot.at = at;
    delete host.dataset.glide;
    return;
  }
  const token = {};
  slot.motionToken = token;
  slot.at = 'moving';
  host.dataset.glide = '';
  const base = slot.base ?? { width: to.width, height: to.height };
  slot.base = base;
  const easing = 'cubic-bezier(0.22, 0.8, 0.2, 1)';
  try {
    const flight = host.animate(
      [
        { left: `${from.left}px`, top: `${from.top}px`, width: `${from.width}px`, height: `${from.height}px` },
        { left: `${to.left}px`, top: `${to.top}px`, width: `${to.width}px`, height: `${to.height}px` },
      ],
      { duration: GLIDE_MS, easing, fill: 'both' },
    );
    fitMotion = fit.animate([{ transform: fitTransform(base, from) }, { transform: fitTransform(base, to) }], {
      duration: GLIDE_MS,
      easing,
      fill: 'both',
    });
    slot.motion = flight;
    flight.onfinish = () => {
      if (slot.motionToken !== token || live !== slot) return;
      paint(slot, to);
      stopMotion(slot);
      slot.at = at;
      delete host.dataset.glide;
      if (at === 'card') followCard(slot);
    };
  } catch {
    paint(slot, to);
    slot.at = at;
    delete host.dataset.glide;
  }
}

function followCard(slot: Slot): void {
  const home = slot.home;
  if (!(home instanceof HTMLElement) || !home.isConnected) return;
  const box = contentBox(home);
  if (!usable(box)) return;
  paint(slot, box);
  slot.at = 'card';
  const host = document.getElementById(PICTURE_ID);
  if (host) host.dataset.at = 'card';
}

/** Seat the picture over its card, once. Later moves only change the seat's box. */
function seatPicture(slot: Slot): void {
  const card = slot.home instanceof HTMLElement ? slot.home : slot.node.parentElement;
  if (!(card instanceof HTMLElement)) return;
  if (card.id !== PICTURE_ID && !card.classList.contains('now-playing-fit')) slot.home = card;
  const measured = card.id === PICTURE_ID || card.classList.contains('now-playing-fit') ? null : contentBox(card);
  const box = measured && usable(measured) ? measured : { left: 0, top: 0, width: 640, height: 360 };
  if (!slot.base) slot.base = { width: box.width, height: box.height };
  const { fit, host } = picture();
  if (slot.node.parentElement !== fit) fit.append(slot.node);
  paint(slot, box);
  slot.at = 'card';
  host.dataset.at = 'card';
  if (slot.unpicture) return;
  let frame = 0;
  const track = () => {
    frame = 0;
    if (live !== slot || slot.at !== 'card') return;
    followCard(slot);
  };
  const schedule = () => {
    if (!frame) frame = requestAnimationFrame(track);
  };
  window.addEventListener('scroll', schedule, { capture: true, passive: true });
  window.addEventListener('resize', schedule);
  slot.unpicture = () => {
    if (frame) cancelAnimationFrame(frame);
    window.removeEventListener('scroll', schedule, { capture: true });
    window.removeEventListener('resize', schedule);
  };
}

/**
 * A focused provider frame swallows every key, and removing one can leave them nowhere until
 * the document reloads. The keyboard goes back to the page first.
 */
function releaseFrameFocus(node: Node): void {
  if (!(node instanceof HTMLIFrameElement) || document.activeElement !== node) return;
  const page = document.getElementById('content');
  if (page instanceof HTMLElement) page.focus({ preventScroll: true });
  else node.blur();
}

function dropPicture(slot: Slot): void {
  releaseFrameFocus(slot.node);
  slot.unpicture?.();
  slot.unpicture = undefined;
  stopMotion(slot);
  if (!live || live === slot) document.getElementById(PICTURE_ID)?.remove();
}

/** The position the bar shows. A quiet provider and a clipped element both keep the seconds moving. */
function shown(slot: Slot): PlaybackState {
  const snap = slot.snapshot;
  if (!snap) return { phase: 'idle', position: 0, speed: NaN, progress: 0, seekable: false, volume: 1, muted: false, volumeAdjustable: false };
  if (snap.phase !== 'playing') return snap;
  let position = snap.position;
  if (slot.audio && Number.isFinite(slot.audio.currentTime)) position = slot.audio.currentTime;
  else if (slot.sampleAt && Number.isFinite(snap.position)) {
    const speed = Number.isFinite(snap.speed) && snap.speed > 0 ? snap.speed : 1;
    position = snap.position + ((performance.now() - slot.sampleAt) / 1000) * speed;
  }
  if (!Number.isFinite(position)) return snap;
  if (position < 0) position = 0;
  if (snap.duration && position > snap.duration) position = snap.duration;
  if (position === snap.position) return snap;
  const progress = snap.duration ? Math.min(1, position / snap.duration) : snap.progress;
  return { ...snap, position, progress, ...endsAt({ ...snap, position }) };
}

function ensureClock(): void {
  if (clockHandle || typeof window === 'undefined') return;
  clockHandle = window.setInterval(() => {
    const slot = live;
    const snap = slot?.snapshot;
    if (!slot || !snap || snap.phase !== 'playing') return;
    const state = shown(slot);
    const second = Math.floor(state.position);
    if (!Number.isFinite(second) || second === slot.clocked) return;
    slot.clocked = second;
    publish();
  }, 250);
}

function publish(): void {
  const slot = live;
  if (slot?.snapshot && slot.snapshot.phase !== 'idle') syncMediaSession(slot);
  const next = playing();
  for (const listener of listeners) listener(next);
}

export function playing(): Session | null {
  if (!live?.snapshot || live.snapshot.phase === 'idle') return null;
  return { episode: live.episode, state: shown(live), visual: live.visual, controls: live.controls };
}

export function visualPlaying(src: string): boolean {
  return Boolean(src && live?.visual && live.live && live.episode.src === src);
}

export function subscribe(listener: (session: Session | null) => void): () => void {
  listeners.add(listener);
  listener(playing());
  return () => listeners.delete(listener);
}

function escapeAttr(value: string): string {
  if (typeof CSS !== 'undefined' && typeof CSS.escape === 'function') return CSS.escape(value);
  return value.replace(/["\\]/g, '\\$&');
}

/** Cards showing this recording: the marked one, or a not-yet-enhanced `<audio>` with the same src. */
function cardsFor(src: string): HTMLElement[] {
  if (typeof document === 'undefined' || !src) return [];
  const found = new Set<HTMLElement>();
  for (const card of document.querySelectorAll(`[data-playing-src="${escapeAttr(src)}"]`)) {
    if (card instanceof HTMLElement) found.add(card);
  }
  const parked = `#${HOST_ID}, #${PARK_ID}, #${PICTURE_ID}, .now-playing`;
  for (const audio of document.querySelectorAll('audio')) {
    if (!(audio instanceof HTMLAudioElement) || audio.src !== src || audio.closest(parked)) continue;
    const card = audio.closest('[data-audio-component]');
    if (card instanceof HTMLElement) found.add(card);
  }
  for (const video of document.querySelectorAll('video')) {
    if (!(video instanceof HTMLVideoElement) || video.src !== src || video.closest(parked)) continue;
    const card = video.closest('.native-video');
    if (card instanceof HTMLElement) found.add(card);
  }
  return [...found];
}

function inView(card: HTMLElement): boolean {
  const rect = card.getBoundingClientRect();
  return (
    rect.height > 0 &&
    rect.bottom > 0 &&
    rect.top < window.innerHeight &&
    rect.right > 0 &&
    rect.left < window.innerWidth
  );
}

/** True when a card for this recording is in the viewport. A zero-height box (jsdom) is not. */
export function cardInView(src: string): boolean {
  return cardsFor(src).some(inView);
}

/**
 * Report whether the episode card is on screen. The observer is a wake-up; the rectangle is
 * the decision, so a test can stand in for layout and a card inserted during navigation is
 * measured before the next paint.
 */
export function watchOnScreen(src: string, report: (visible: boolean) => void): () => void {
  let stopped = false;
  const scan = () => {
    if (!stopped) report(cardInView(src));
  };
  const observed = new Set<Element>();
  let observer: IntersectionObserver | null = null;
  if (typeof IntersectionObserver === 'function') {
    observer = new IntersectionObserver(() => scan());
  }
  const watchCards = () => {
    if (!observer) return;
    const current = new Set(cardsFor(src));
    for (const card of current) {
      if (!observed.has(card)) {
        observed.add(card);
        observer.observe(card);
      }
    }
    for (const card of observed) {
      if (!current.has(card as HTMLElement) || !card.isConnected) {
        observed.delete(card);
        observer.unobserve(card);
      }
    }
  };
  const onChange = () => {
    watchCards();
    scan();
  };
  const mutations = new MutationObserver(onChange);
  mutations.observe(document.body, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ['data-playing-src'],
  });
  window.addEventListener('scroll', onChange, { passive: true });
  window.addEventListener('resize', onChange);
  watchCards();
  scan();
  return () => {
    stopped = true;
    observer?.disconnect();
    mutations.disconnect();
    window.removeEventListener('scroll', onChange);
    window.removeEventListener('resize', onChange);
  };
}

function positionOf(slot: Slot): number {
  if (slot.audio && Number.isFinite(slot.audio.currentTime)) return slot.audio.currentTime;
  return slot.snapshot?.position ?? NaN;
}

function lengthOf(slot: Slot): number | undefined {
  if (slot.audio) return duration(slot.audio.duration) ?? duration(slot.snapshot?.duration);
  return duration(slot.snapshot?.duration);
}

function remember(slot: Slot, force: boolean): void {
  const src = slot.episode.src;
  const time = positionOf(slot);
  if (!src || !Number.isFinite(time) || slot.snapshot?.phase === 'idle') return;
  // A tick at the start must not erase a position saved on an earlier visit.
  if (!force && time < 1) return;
  const now = Date.now();
  if (!force && now - slot.savedAt < 1000) return;
  slot.savedAt = now;
  try {
    storage()?.setItem(RESUME_PREFIX + src, String(time));
  } catch {
    /* storage blocked */
  }
}

/** Offer a saved position once the file's own length is known. Skip the start and the last moments. */
function tryResume(slot: Slot): void {
  if (slot.resumed) return;
  const total = lengthOf(slot);
  if (!total) return;
  slot.resumed = true;
  const saved = Number(storage()?.getItem(RESUME_PREFIX + slot.episode.src));
  if (!Number.isFinite(saved) || saved < 1 || saved >= total - 2) return;
  // A player already underway is the same element continuing, not a fresh start.
  const time = positionOf(slot);
  if (Number.isFinite(time) && time >= 1) return;
  try {
    if (slot.audio) slot.audio.currentTime = saved;
    else slot.playback.seek(saved / total);
  } catch {
    slot.resumed = false;
  }
}

function applyRemembered(slot: Slot): void {
  const { audio } = slot;
  if (!audio) {
    const rate = slot.snapshot?.speed;
    if (rememberedRate != null && rememberedRate !== 1 && (!Number.isFinite(rate) || rate === 1)) {
      slot.playback.speed(rememberedRate);
    }
    return;
  }
  if (rememberedRate != null && audio.playbackRate === 1 && rememberedRate !== 1) slot.playback.speed(rememberedRate);
  if (!rememberedVolume || audio.volume !== 1 || audio.muted) return;
  if (rememberedVolume.muted || rememberedVolume.volume === 0) slot.playback.mute();
  else if (rememberedVolume.volume !== 1) slot.playback.volume(rememberedVolume.volume);
}

function notePreferences(slot: Slot): void {
  if (!slot.settled) return;
  if (!slot.audio) {
    const rate = slot.snapshot?.speed;
    if (rate !== undefined && Number.isFinite(rate)) rememberedRate = rate;
    return;
  }
  rememberedRate = slot.audio.playbackRate;
  rememberedVolume = {
    volume: slot.audio.volume,
    muted: slot.audio.muted || slot.audio.volume === 0,
  };
}

function mediaSession(): MediaSession | null {
  try {
    return navigator.mediaSession ?? null;
  } catch {
    return null;
  }
}

/** Playing or still starting. A second play must not pause, and a second pause must not play. */
function playingNow(slot: Slot): boolean {
  const phase = slot.snapshot?.phase;
  if (phase === 'playing' || phase === 'loading') return true;
  return Boolean(slot.audio && !slot.audio.paused);
}

/**
 * A hardware key can arrive both as a media-session action and as a `Media*` keydown.
 * The same command inside this window is one press. A different command still goes through.
 */
let lastAct = '';
let lastActAt = 0;

function guard(name: string): boolean {
  const now = typeof performance === 'undefined' ? Date.now() : performance.now();
  if (name === lastAct && now - lastActAt < 80) return false;
  lastAct = name;
  lastActAt = now;
  return true;
}

function playSlot(slot: Slot): void {
  if (!slot.live || !slot.controls || playingNow(slot) || !guard('play')) return;
  slot.playback.toggle();
}

function pauseSlot(slot: Slot): void {
  if (!slot.live || !slot.controls || !playingNow(slot) || !guard('pause')) return;
  slot.playback.toggle();
}

function skipSlot(slot: Slot, seconds: number): void {
  if (!slot.live || !slot.controls || !Number.isFinite(seconds) || !guard(`skip:${seconds}`)) return;
  slot.playback.skip(seconds);
}

function seekSlot(slot: Slot, seconds: number): void {
  if (!slot.live || !slot.controls || !Number.isFinite(seconds)) return;
  const total = lengthOf(slot);
  if (!total) return;
  const clamped = Math.max(0, Math.min(total, seconds));
  if (!guard(`seek:${clamped}`)) return;
  slot.playback.seek(clamped / total);
}

function skipOffset(offset: number | null | undefined, fallback: number): number {
  return typeof offset === 'number' && Number.isFinite(offset) && offset > 0 ? offset : fallback;
}

const MEDIA_ACTIONS = ['play', 'pause', 'seekbackward', 'seekforward', 'seekto', 'previoustrack', 'nexttrack', 'stop'] as const;

let described: { title: string; show: string; artwork: string } | null = null;

function describe(slot: Slot): void {
  const session = mediaSession();
  if (!session || typeof MediaMetadata !== 'function') return;
  const next = { title: slot.episode.title, show: slot.episode.show, artwork: slot.episode.artwork };
  if (described && described.title === next.title && described.show === next.show && described.artwork === next.artwork) return;
  described = next;
  try {
    session.metadata = new MediaMetadata({
      title: next.title,
      artist: next.show,
      artwork: next.artwork ? [{ src: next.artwork, sizes: '512x512' }] : [],
    });
  } catch {
    described = null;
  }
}

function reportPosition(session: MediaSession, slot: Slot): void {
  if (!session.setPositionState) return;
  const state = shown(slot);
  let position = Number.isFinite(state.position) && state.position >= 0 ? state.position : 0;
  const rate = Number.isFinite(state.speed) && state.speed > 0 ? state.speed : 1;
  const total = lengthOf(slot);
  const reported: MediaPositionState = { playbackRate: rate, position };
  if (total) {
    if (position > total) position = total;
    reported.position = position;
    reported.duration = total;
  }
  try {
    session.setPositionState(reported);
  } catch {
    /* an unfinished length is not a position the platform can show */
  }
}

function syncMediaSession(slot: Slot): void {
  const session = mediaSession();
  // A frame the bar cannot command keeps the keys it already owns.
  if (!session || !slot.controls) return;
  describe(slot);
  const phase = slot.snapshot?.phase;
  try {
    session.playbackState = phase === 'playing' || phase === 'loading' ? 'playing' : 'paused';
  } catch {
    /* the platform keeps its own state */
  }
  reportPosition(session, slot);
}

function bindMediaSession(slot: Slot): void {
  const session = mediaSession();
  if (!session?.setActionHandler) return;
  if (!slot.controls) {
    clearMediaSession();
    return;
  }
  describe(slot);
  const set = (action: MediaSessionAction, handler: MediaSessionActionHandler | null) => {
    try {
      session.setActionHandler(action, handler);
    } catch {
      /* this action is not supported */
    }
  };
  set('play', () => playSlot(slot));
  set('pause', () => pauseSlot(slot));
  set('seekbackward', (details) => skipSlot(slot, -skipOffset(details.seekOffset, 15)));
  set('seekforward', (details) => skipSlot(slot, skipOffset(details.seekOffset, 30)));
  set('previoustrack', () => skipSlot(slot, -15));
  set('nexttrack', () => skipSlot(slot, 30));
  set('seekto', (details) => {
    if (details.seekTime != null) seekSlot(slot, details.seekTime);
  });
  set('stop', () => {
    if (!guard('stop')) return;
    dismiss();
  });
  syncMediaSession(slot);
}

function clearMediaSession(): void {
  described = null;
  const session = mediaSession();
  if (!session?.setActionHandler) return;
  for (const action of MEDIA_ACTIONS) {
    try {
      session.setActionHandler(action, null);
    } catch {
      /* already clear */
    }
  }
  try {
    session.metadata = null;
    session.playbackState = 'none';
  } catch {
    /* nothing to clear */
  }
  try {
    session.setPositionState();
  } catch {
    /* already clear */
  }
}

function onMediaKey(event: KeyboardEvent): void {
  const slot = live;
  if (!slot?.live || !slot.controls) return;
  const act = mediaKey(event.key);
  if (!act) return;
  event.preventDefault();
  act(slot);
}

function mediaKey(key: string): ((slot: Slot) => void) | null {
  switch (key) {
    case 'MediaPlayPause':
      return (slot) => (playingNow(slot) ? pauseSlot(slot) : playSlot(slot));
    case 'MediaPlay':
      return playSlot;
    case 'MediaPause':
      return pauseSlot;
    case 'MediaStop':
      return () => {
        if (!guard('stop')) return;
        dismiss();
      };
    case 'MediaTrackNext':
      return (slot) => skipSlot(slot, 30);
    case 'MediaTrackPrevious':
      return (slot) => skipSlot(slot, -15);
    default:
      return null;
  }
}

function showVisual(home: Element, node: HTMLElement): void {
  if (!(home instanceof HTMLElement) || !(node instanceof HTMLIFrameElement)) return;
  const preview = home.querySelector<HTMLElement>('.video-preview');
  if (preview) preview.hidden = true;
  home.classList.add('is-loaded');
  home.classList.remove('is-loading');
  document.getElementById(PICTURE_ID)?.classList.remove('is-loading');
  home.dataset.videoMounted = 'true';
}

function restore(slot: Slot): void {
  if (slot.home?.isConnected) slot.home.insertBefore(slot.node, slot.anchor);
  slot.live = false;
  if (live === slot) {
    live = null;
    clearMediaSession();
  }
  publish();
}

function releaseNode(slot: Slot): void {
  const node = slot.node;
  if (node instanceof HTMLMediaElement) {
    releaseElement(node);
    return;
  }
  releaseFrameFocus(node);
  node.removeAttribute('src');
  const player = slot.home;
  node.remove();
  if (!(player instanceof HTMLElement) || !player.isConnected) return;
  const preview = player.querySelector<HTMLElement>('.video-preview');
  if (preview) preview.hidden = false;
  player.classList.remove('is-loaded', 'is-loading');
  player.removeAttribute('aria-busy');
  delete player.dataset.videoMounted;
}

function retire(slot: Slot, quiet = false): void {
  if (slot.snapshot) slot.render({ ...slot.snapshot, phase: 'paused', endsAt: undefined });
  slot.live = false;
  remember(slot, true);
  dropPicture(slot);
  if (slot.node instanceof HTMLIFrameElement) slot.node.remove();
  else if (slot.home?.isConnected) slot.home.insertBefore(slot.node, slot.anchor);
  else if (slot.node.parentElement?.id === HOST_ID || slot.node.parentElement?.id === PARK_ID || slot.node.closest('.now-playing') || slot.node.closest(`#${PICTURE_ID}`)) {
    slot.node.remove();
  }
  slot.playback.dispose();
  releaseNode(slot);
  if (live === slot) {
    live = null;
    if (!quiet) clearMediaSession();
  }
  if (!quiet) publish();
}

function promote(slot: Slot): void {
  if (slot.live) return;
  if (live && live !== slot) retire(live, true);
  slot.home = slot.node.parentElement;
  slot.anchor = slot.node.nextSibling;
  slot.live = true;
  slot.settled = false;
  live = slot;
  // Audio cannot be seen, so it leaves the card immediately. A picture is seated once, over the
  // card, and only the seat moves after that.
  if (slot.visual) seatPicture(slot);
  else host().append(slot.node);
  ensureClock();
  tryResume(slot);
  applyRemembered(slot);
  slot.settled = true;
  bindMediaSession(slot);
  publish();
}

function accept(slot: Slot, state: PlaybackState): void {
  if (Number.isFinite(state.position) && state.position !== slot.snapshot?.position) slot.sampleAt = performance.now();
  slot.snapshot = state;
  slot.render(state);
  if (!slot.live) {
    if (state.phase === 'playing' || state.phase === 'loading') promote(slot);
    return;
  }
  // A visual error keeps the session. Restoring it would drop the live frame, and the next
  // page change would destroy it.
  if (!slot.visual && state.phase === 'error' && slot.home?.isConnected) {
    restore(slot);
    return;
  }
  if (!slot.resumed) tryResume(slot);
  if (state.phase !== 'idle') remember(slot, state.phase === 'paused' || state.phase === 'ended');
  notePreferences(slot);
  publish();
}

/** The page is about to destroy the article. The picture is already outside it; forget the card. */
function detachFromPage(slot: Slot): void {
  if (!slot.visual || !slot.live) return;
  const home = slot.home;
  if (home?.isConnected && home.contains(slot.node)) seatPicture(slot);
  slot.home = null;
  slot.anchor = null;
}

/**
 * Move the seat onto the bar's stage. The picture element stays where it was seated. Cleanup
 * sends the seat back to the card when the bar hides and that card is still on screen.
 */
export function placeVisual(stage: HTMLElement): () => void {
  const slot = live;
  if (!slot?.visual || !slot.live) return () => {};
  let aimed: Box | null = null;
  let retry = 0;
  let retried = false;
  const go = () => {
    if (live !== slot || !slot.live) return;
    const node = document.getElementById(PICTURE_ID);
    const box = contentBox(stage);
    if (!usable(box)) {
      // The bar can mount before it has a box. Mark the chrome, but leave `slot.at` so the
      // next real measurement still glides instead of snapping.
      if (node) node.dataset.at = 'dock';
      if (!retried) {
        retried = true;
        retry = requestAnimationFrame(go);
      }
      return;
    }
    const headingDock = slot.at === 'dock' || (slot.at === 'moving' && node?.dataset.at === 'dock');
    if (headingDock && aimed && near(aimed, box)) return;
    aimed = box;
    if (slot.at === 'dock') paint(slot, box);
    else glide(slot, box, 'dock');
  };
  go();
  let observer: ResizeObserver | null = null;
  if (typeof ResizeObserver === 'function') {
    observer = new ResizeObserver(go);
    observer.observe(stage);
  }
  return () => {
    if (retry) cancelAnimationFrame(retry);
    observer?.disconnect();
    if (live !== slot || !slot.live) return;
    const home = slot.home;
    if (!(home instanceof HTMLElement) || !home.isConnected || !inView(home)) return;
    const box = contentBox(home);
    if (!usable(box)) return;
    showVisual(home, slot.node);
    glide(slot, box, 'card');
  };
}

/**
 * A returned article for the recording already playing. The new card's own element is dropped;
 * the live one takes its place when the card is on screen.
 */
export function claimVisual(
  home: HTMLElement,
  episode: Episode,
  render: (state: PlaybackState) => void,
  signal: AbortSignal,
): boolean {
  const slot = live;
  if (!slot?.visual || !slot.live || slot.episode.src !== episode.src || signal.aborted) return false;
  const previous = slot.render;
  slot.episode = episode;
  slot.home = home;
  slot.anchor = null;
  slot.render = (state) => {
    previous(state);
    if (!signal.aborted) render(state);
  };
  home.dataset.playingSrc = episode.src;
  for (const child of [...home.querySelectorAll('video, iframe')]) {
    if (child === slot.node) continue;
    if (child instanceof HTMLMediaElement) releaseElement(child);
    else if (child instanceof HTMLIFrameElement) child.removeAttribute('src');
    child.remove();
  }
  if (inView(home)) {
    showVisual(home, slot.node);
    const box = contentBox(home);
    if (usable(box)) glide(slot, box, 'card');
  }
  signal.addEventListener(
    'abort',
    () => {
      if (live !== slot) return;
      slot.render = previous;
      if (slot.home === home) detachFromPage(slot);
    },
    { once: true },
  );
  publish();
  return true;
}

/**
 * A picture the reader has started. `playback` drives it; `push` is its state. The page signal
 * parks a started player and releases one that never left idle.
 */
export function connectVisual(
  node: HTMLElement,
  episode: Episode,
  playback: Playback,
  render: (state: PlaybackState) => void,
  signal: AbortSignal,
  options: { controls: boolean },
): { push: (state: PlaybackState) => void } {
  ensurePagehide();
  const media = node instanceof HTMLMediaElement ? node : null;
  if (signal.aborted) {
    playback.dispose();
    if (media) releaseElement(media);
    else {
      releaseFrameFocus(node);
      node.removeAttribute('src');
      node.remove();
    }
    return { push() {} };
  }

  const slot: Slot = {
    audio: media,
    node,
    visual: true,
    controls: options.controls,
    episode,
    playback,
    render,
    snapshot: null,
    live: false,
    settled: false,
    resumed: false,
    savedAt: 0,
    home: null,
    anchor: null,
    base: null,
    at: 'card',
    motion: null,
    motionToken: null,
    sampleAt: 0,
    clocked: -1,
  };
  const push = (state: PlaybackState) => accept(slot, state);
  signal.addEventListener(
    'abort',
    () => {
      if (slot.live) {
        if (slot.render === render) slot.render = noop;
        detachFromPage(slot);
        return;
      }
      slot.playback.dispose();
      releaseNode(slot);
    },
    { once: true },
  );
  if (media && !media.paused) promote(slot);
  return { push };
}

/** The duplicate card yields its place, so a later failure can put the real element back there. */
function takePlaceOf(slot: Slot, duplicate: HTMLAudioElement): void {
  const parent = duplicate.parentElement;
  const anchor = duplicate.nextSibling;
  releaseElement(duplicate);
  if (!parent) return;
  slot.home = parent;
  slot.anchor = anchor;
  duplicate.remove();
}

function ensurePagehide(): void {
  if (pagehideBound || typeof window === 'undefined') return;
  pagehideBound = true;
  // Leaving the site, not an in-app page change: `pagehide` does not fire for those.
  window.addEventListener('pagehide', () => dismiss());
  // Hardware keys. Left in place for the life of the page, like `pagehide`: a test reset
  // drops the recording, and the next one finds the listener still here.
  window.addEventListener('keydown', onMediaKey, true);
  ensureClock();
}

function sameRecording(a: HTMLMediaElement, b: HTMLMediaElement): boolean {
  return a.src !== '' && a.src === b.src;
}

/**
 * Bind a card to its recording. A second card for the recording already playing attaches to
 * that element; anything else stays local until its own play promotes it. `signal` only
 * unbinds a promoted recording — disposing it would stop the audio the reader just parked.
 */
export function connect(
  audio: HTMLAudioElement,
  episode: Episode,
  known: { duration?: number },
  render: (state: PlaybackState) => void,
  signal: AbortSignal,
): { playback: Playback; audio: HTMLAudioElement; snapshot: PlaybackState | null } {
  ensurePagehide();
  if (signal.aborted) {
    releaseElement(audio);
    return { playback: inertPlayback(), audio, snapshot: null };
  }

  if (live?.audio instanceof HTMLAudioElement && live.audio !== audio && sameRecording(live.audio, audio)) {
    takePlaceOf(live, audio);
    live.render = render;
    live.episode = episode;
    signal.addEventListener(
      'abort',
      () => {
        if (live?.render === render) live.render = noop;
      },
      { once: true },
    );
    publish();
    return { playback: live.playback, audio: live.audio, snapshot: live.snapshot };
  }

  if (live?.audio === audio) {
    live.render = render;
    live.episode = episode;
    signal.addEventListener(
      'abort',
      () => {
        if (live?.audio === audio && live.render === render) live.render = noop;
      },
      { once: true },
    );
    publish();
    return { playback: live.playback, audio, snapshot: live.snapshot };
  }

  const slot: Slot = {
    audio,
    node: audio,
    visual: false,
    controls: true,
    episode,
    playback: inertPlayback(),
    render,
    snapshot: null,
    live: false,
    settled: false,
    resumed: false,
    savedAt: 0,
    home: null,
    anchor: null,
    base: null,
    at: 'card',
    motion: null,
    motionToken: null,
    sampleAt: 0,
    clocked: -1,
  };
  slot.playback = createPlayback(audio, (state) => accept(slot, state), known);

  const playback: Playback = {
    toggle() {
      if (!slot.live && audio.paused && slot.snapshot?.phase !== 'loading') promote(slot);
      slot.playback.toggle();
    },
    seek: (fraction) => slot.playback.seek(fraction),
    skip: (seconds) => slot.playback.skip(seconds),
    speed: (value) => slot.playback.speed(value),
    volume: (value) => slot.playback.volume(value),
    mute: () => slot.playback.mute(),
    dispose: () => slot.playback.dispose(),
  };

  signal.addEventListener(
    'abort',
    () => {
      if (slot.live) {
        if (slot.render === render) slot.render = noop;
        return;
      }
      slot.playback.dispose();
      releaseElement(audio);
    },
    { once: true },
  );

  if (!audio.paused) promote(slot);
  return { playback, audio, snapshot: slot.snapshot };
}

function nextDockSpeed(current: number): number {
  const index = DOCK_SPEEDS.indexOf(current);
  if (index >= 0) return DOCK_SPEEDS[(index + 1) % DOCK_SPEEDS.length];
  return DOCK_SPEEDS.find((speed) => speed > current) ?? DOCK_SPEEDS[0];
}

export function togglePlayback(): void {
  live?.playback.toggle();
}

export function skipPlayback(seconds: number): void {
  live?.playback.skip(seconds);
}

export function seekPlayback(fraction: number): void {
  live?.playback.seek(fraction);
}

export function mutePlayback(): void {
  if (!live || live.visual) return;
  live.playback.mute();
}

export function volumePlayback(value: number): void {
  if (!live || live.visual) return;
  live.playback.volume(value);
}

export function cycleSpeed(): void {
  if (!live?.controls) return;
  const reported = live.snapshot?.speed;
  const rate = live.audio?.playbackRate ?? (reported !== undefined && Number.isFinite(reported) ? reported : 1);
  live.playback.speed(nextDockSpeed(rate));
}

/** Stop the recording and drop the bar. */
export function dismiss(): void {
  if (live) retire(live);
}

/** Test isolation: drop the parked element and the host. Preferences do not survive it. */
export function resetPlayback(): void {
  if (live) {
    live.unpicture?.();
    stopMotion(live);
    live.live = false;
    live.playback.dispose();
    if (live.visual) {
      releaseFrameFocus(live.node);
      live.node.remove();
    }
    live = null;
  }
  rememberedRate = null;
  rememberedVolume = null;
  lastAct = '';
  lastActAt = 0;
  if (typeof document !== 'undefined') {
    document.getElementById(HOST_ID)?.remove();
    document.getElementById(PARK_ID)?.remove();
    document.getElementById(PICTURE_ID)?.remove();
  }
  clearMediaSession();
  publish();
}
