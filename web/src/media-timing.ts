export interface PlaybackTimingState {
  phase: string;
  position: number;
  duration?: number;
  speed: number;
}

export function finiteDuration(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) && value > 0 && value <= Number.MAX_SAFE_INTEGER ? value : undefined;
}

export function playbackTiming(state: PlaybackTimingState, nowMs = Date.now()): { remainingSeconds?: number; endsAt?: number } {
  const duration = finiteDuration(state.duration), speed = finiteDuration(state.speed);
  if (!duration || !speed || !Number.isFinite(state.position)) return {};
  const remainingSeconds = Math.max(0, duration - Math.max(0, state.position)) / speed;
  const end = nowMs + remainingSeconds * 1000;
  const endsAt = state.phase === "playing" && remainingSeconds > 0 && Number.isFinite(end) && Math.abs(end) <= 8.64e15 ? end : undefined;
  return { remainingSeconds, ...(endsAt === undefined ? {} : { endsAt }) };
}

export function formatPlaybackEnd(endsAt: number): string {
  const date = new Date(endsAt);
  return Number.isFinite(date.getTime()) ? `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}` : "";
}

export function updateConsumption(root: ParentNode, action: "listen" | "watch", seconds: number) {
  if (!finiteDuration(seconds)) return;
  const target = root.querySelector<HTMLElement>(`.itemhead .reading-stats[data-consumption="${action}"]`);
  if (!target) return;
  target.removeAttribute("title");
  if (target.dataset.durationSeconds === String(seconds)) return;
  const time = target.ownerDocument.createElement("time");
  time.dateTime = `PT${seconds}S`;
  time.textContent = `${Math.max(1, Math.ceil(seconds / 60))} min ${action}`;
  target.dataset.durationSeconds = String(seconds);
  target.replaceChildren(time);
}

export interface NativeTimingPort extends EventTarget {
  duration: number;
  currentTime: number;
  playbackRate: number;
  paused: boolean;
  ended: boolean;
  readyState: number;
  pause(): void;
}

export function createNativeTiming(media: NativeTimingPort, update: (state: PlaybackTimingState) => void, archivedDuration?: number) {
  const listeners = new AbortController();
  let phase = media.paused ? "paused" : media.readyState >= 3 ? "playing" : "loading";
  function emit() {
    if (listeners.signal.aborted) return;
    const duration = Number.isNaN(media.duration) ? finiteDuration(archivedDuration) : finiteDuration(media.duration);
    update({ phase, duration, position: media.currentTime, speed: media.playbackRate });
  }
  function setPhase(next: string) { phase = next; emit(); }
  for (const event of ["loadedmetadata", "durationchange", "timeupdate", "ratechange"]) media.addEventListener(event, emit, { signal: listeners.signal });
  media.addEventListener("play", () => setPhase("loading"), { signal: listeners.signal });
  media.addEventListener("playing", () => setPhase("playing"), { signal: listeners.signal });
  media.addEventListener("waiting", () => setPhase(media.paused ? "paused" : "loading"), { signal: listeners.signal });
  media.addEventListener("seeking", () => setPhase("loading"), { signal: listeners.signal });
  media.addEventListener("seeked", () => setPhase(media.paused ? "paused" : media.readyState >= 3 ? "playing" : "loading"), { signal: listeners.signal });
  media.addEventListener("pause", () => setPhase(media.ended ? "ended" : "paused"), { signal: listeners.signal });
  media.addEventListener("ended", () => setPhase("ended"), { signal: listeners.signal });
  media.addEventListener("error", () => setPhase("error"), { signal: listeners.signal });
  media.addEventListener("emptied", () => setPhase("idle"), { signal: listeners.signal });
  emit();
  return { dispose() { listeners.abort(); media.pause(); } };
}

interface ProviderTimingPorts {
  provider: "youtube" | "vimeo";
  origin: string;
  source(): unknown;
  post(message: object): void;
  update(state: PlaybackTimingState): void;
  duration?: number;
}

const vimeoEvents = ["play", "playing", "pause", "ended", "timeupdate", "playbackratechange", "bufferstart", "bufferend", "seeking", "seeked", "durationchange", "error"];
function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
}
function messageData(value: unknown) {
  if (typeof value === "string") {
    if (value.length > 128_000) return;
    try { return record(JSON.parse(value)); } catch { return; }
  }
  return record(value);
}
function youtubePhase(value: unknown): string | undefined {
  return value === 1 ? "playing" : value === 2 ? "paused" : value === 3 ? "loading" : value === 0 ? "ended" : value === -1 || value === 5 ? "idle" : undefined;
}

export function createProviderTiming(ports: ProviderTimingPorts) {
  let state: PlaybackTimingState = { phase: "idle", position: NaN, duration: finiteDuration(ports.duration), speed: NaN };
  let recorded = !!state.duration;
  let live = false;
  let disposed = false;
  let subscribed = false;
  let vimeoPlayback = "idle";
  let stale = false;
  let watchdog: ReturnType<typeof setTimeout> | undefined;
  function emit() {
    clearTimeout(watchdog);
    ports.update({ ...state });
    if (state.phase === "playing" && state.duration) {
      watchdog = setTimeout(() => {
        if (disposed) return;
        stale = true;
        state = { ...state, phase: "loading" };
        ports.update({ ...state });
      }, 5000);
    }
  }
  function post(message: object) { if (!disposed) ports.post(message); }
  function loaded() {
    if (disposed) return;
    if (ports.provider === "youtube") {
      post({ event: "listening", id: "aggr-timing", channel: "widget" });
      for (const event of ["onStateChange", "onPlaybackRateChange", "onError"]) post({ event: "command", func: "addEventListener", args: [event], id: "aggr-timing", channel: "widget" });
    } else {
      for (const event of vimeoEvents) post({ method: "addEventListener", value: event });
      for (const method of ["getDuration", "getCurrentTime", "getPlaybackRate", "getPaused"]) post({ method });
    }
    subscribed = true;
  }
  function message(event: { source: unknown; origin: string; data: unknown }) {
    if (disposed || !ports.source() || event.source !== ports.source() || event.origin !== ports.origin) return;
    const data = messageData(event.data);
    if (!data) return;
    if (ports.provider === "youtube") {
      if (data.event === "readyToListen" || data.event === "onReady") { loaded(); return; }
      if (data.event === "initialDelivery" || data.event === "infoDelivery") {
        const info = record(data.info);
        if (!info) return;
        const video = record(info.videoData);
        if (typeof video?.isLive === "boolean") { live = video.isLive; recorded = !live; }
        if (live) state.duration = undefined;
        else if (recorded && info.duration !== undefined) state.duration = finiteDuration(info.duration);
        if (typeof info.currentTime === "number" && Number.isFinite(info.currentTime) && info.currentTime >= 0) {
          state.position = info.currentTime;
          if (stale) state.phase = "playing";
          stale = false;
        }
        if (info.playbackRate !== undefined) state.speed = finiteDuration(info.playbackRate) ?? NaN;
        const phase = youtubePhase(info.playerState);
        if (phase) { state.phase = phase; stale = false; }
      } else if (data.event === "onStateChange") { state.phase = youtubePhase(data.info) ?? "idle"; stale = false; }
      else if (data.event === "onPlaybackRateChange") state.speed = finiteDuration(data.info) ?? NaN;
      else if (data.event === "onError") { state.phase = "error"; stale = false; }
      else return;
    } else {
      if (data.event === "ready") { if (!subscribed) loaded(); return; }
      const info = record(data.data);
      if (data.method === "getDuration") state.duration = finiteDuration(data.value);
      else if (data.method === "getCurrentTime" && typeof data.value === "number" && Number.isFinite(data.value) && data.value >= 0) state.position = data.value;
      else if (data.method === "getPlaybackRate") state.speed = finiteDuration(data.value) ?? NaN;
      else if (data.method === "getPaused") { if (data.value === true) { state.phase = vimeoPlayback = "paused"; stale = false; } }
      else if (typeof data.event === "string" && vimeoEvents.includes(data.event)) {
        if (info?.duration !== undefined) state.duration = finiteDuration(info.duration);
        if (typeof info?.seconds === "number" && Number.isFinite(info.seconds) && info.seconds >= 0) {
          state.position = info.seconds;
          if (data.event === "timeupdate" && stale) { state.phase = vimeoPlayback; stale = false; }
        }
        if (!["timeupdate", "playbackratechange", "durationchange"].includes(data.event)) stale = false;
        if (info?.playbackRate !== undefined) state.speed = finiteDuration(info.playbackRate) ?? NaN;
        if (data.event === "playing") state.phase = vimeoPlayback = "playing";
        else if (data.event === "play" || data.event === "bufferstart" || data.event === "seeking") state.phase = "loading";
        else if (data.event === "seeked" || data.event === "bufferend") state.phase = vimeoPlayback;
        else if (data.event === "pause") state.phase = vimeoPlayback = "paused";
        else if (data.event === "ended") state.phase = vimeoPlayback = "ended";
        else if (data.event === "error") state.phase = vimeoPlayback = "error";
      } else return;
    }
    emit();
  }
  return { loaded, message, dispose() { disposed = true; clearTimeout(watchdog); } };
}
