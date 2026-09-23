// @ts-check
/**
 * Players and their timing readouts, loaded only on article pages that carry media.
 *
 * Every control this file drives is already in the HTML, hidden until enhancement succeeds, so a
 * failure here leaves the reader with working native controls rather than nothing.
 */

const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => Array.from(root.querySelectorAll(selector));

const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2, 2.5, 3];

/** A usable, finite length in seconds, or nothing. */
const duration = (value) =>
  typeof value === "number" && Number.isFinite(value) && value > 0 && value <= Number.MAX_SAFE_INTEGER
    ? value
    : undefined;

function clock(seconds) {
  if (!Number.isFinite(seconds) || seconds < 0) return "—";
  const whole = Math.floor(seconds);
  const hours = Math.floor(whole / 3600);
  const minutes = Math.floor(whole / 60) % 60;
  const pad = (value) => String(value).padStart(2, "0");
  return hours ? hours + ":" + pad(minutes) + ":" + pad(whole % 60) : minutes + ":" + pad(whole % 60);
}

/** When the recording finishes, given where it is and how fast it is playing. */
function endsAt(state, now = Date.now()) {
  const length = duration(state.duration);
  const speed = duration(state.speed);
  if (!length || !speed || !Number.isFinite(state.position)) return {};
  const remaining = Math.max(0, length - Math.max(0, state.position)) / speed;
  const end = now + remaining * 1000;
  return {
    remaining,
    endsAt:
      state.phase === "playing" && remaining > 0 && Number.isFinite(end) && Math.abs(end) <= 8.64e15
        ? end
        : undefined,
  };
}

const wallClock = (timestamp) => {
  const date = new Date(timestamp);
  if (!Number.isFinite(date.getTime())) return "";
  return String(date.getHours()).padStart(2, "0") + ":" + String(date.getMinutes()).padStart(2, "0");
};

/**
 * Correct the article header's "N min listen/watch" once the real recording length is known.
 * Show notes and transcripts must never stand in for a duration.
 */
function updateConsumption(root, action, seconds) {
  if (!duration(seconds)) return;
  const target = $(".itemhead .reading-stats[data-consumption='" + action + "']", root);
  if (!target) return;
  target.removeAttribute("title");
  if (target.dataset.durationSeconds === String(seconds)) return;
  const time = document.createElement("time");
  time.dateTime = "PT" + seconds + "S";
  time.textContent = Math.max(1, Math.ceil(seconds / 60)) + " min " + action;
  target.dataset.durationSeconds = String(seconds);
  target.replaceChildren(time);
}

/* ------------------------------------------------------------------ audio */

/** Drive a native `<audio>` element and report its state back to the rendered controls. */
function createPlayback(audio, update, known = {}) {
  const listeners = new AbortController();
  const signal = listeners.signal;
  let phase = audio.paused ? "idle" : "playing";
  let attempt = 0;
  let disposed = false;
  let volumeAdjustable = true;
  let previousVolume = audio.volume > 0 ? audio.volume : 1;

  const seekable = () => duration(audio.duration);
  const length = () => (audio.duration === Infinity ? undefined : seekable() ?? duration(known.duration));

  function emit() {
    if (disposed) return;
    const total = length();
    const position = Number.isFinite(audio.currentTime)
      ? Math.max(0, Math.min(audio.currentTime, total ?? Infinity))
      : 0;
    if (audio.volume > 0) previousVolume = audio.volume;
    const state = { phase, position, duration: total, speed: audio.playbackRate };
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
  const setPhase = (next) => {
    phase = next;
    emit();
  };

  for (const name of ["loadedmetadata", "durationchange", "timeupdate", "ratechange", "volumechange"])
    audio.addEventListener(name, emit, { signal });
  audio.addEventListener("play", () => setPhase("loading"), { signal });
  audio.addEventListener("playing", () => setPhase("playing"), { signal });
  audio.addEventListener("waiting", () => !audio.paused && setPhase("loading"), { signal });
  audio.addEventListener("pause", () => setPhase(audio.ended ? "ended" : "paused"), { signal });
  audio.addEventListener("ended", () => setPhase("ended"), { signal });
  audio.addEventListener("error", () => setPhase("error"), { signal });

  function seek(fraction) {
    const total = seekable();
    if (disposed || !total || !Number.isFinite(fraction)) return;
    try {
      audio.currentTime = Math.max(0, Math.min(1, fraction)) * total;
      emit();
    } catch {
      /* not seekable yet */
    }
  }

  return {
    toggle() {
      if (disposed) return;
      const token = ++attempt;
      if (!audio.paused || phase === "loading") {
        audio.pause();
        setPhase("paused");
        return;
      }
      if (audio.error) audio.load();
      setPhase("loading");
      try {
        void audio.play().catch(() => {
          if (!disposed && token === attempt) setPhase("error");
        });
      } catch {
        if (!disposed && token === attempt) setPhase("error");
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
    volume(value) {
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
    },
    /** Muting drops the visible level to zero; unmuting restores the level in use before. */
    mute() {
      if (disposed) return;
      try {
        if (audio.muted || audio.volume === 0) {
          audio.muted = false;
          this.volume(previousVolume);
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

const AUDIO_STATUS = {
  error: "Audio unavailable. Try playing again.",
  loading: "Loading audio…",
  playing: "Playing",
  paused: "Paused",
  ended: "Finished",
};

function enhanceAudio(card) {
  const audio = $("audio", card);
  const controls = $("[data-audio-controls]", card);
  const tools = $("[data-audio-tools]", card);
  if (!audio || !controls || !tools) return;

  const article = card.closest("article.item") || document;
  const toggle = $("[data-audio-toggle]", card);
  const seekBar = $("[data-audio-seek]", card);
  const elapsed = $("[data-audio-elapsed]", card);
  const total = $("[data-audio-duration]", card);
  const status = $("[data-audio-status]", card);
  const estimate = $("[data-audio-ends-at]", card);
  const speed = $("[data-audio-speed]", card);
  const mute = $("[data-audio-mute]", card);
  const volume = $("[data-audio-volume]", card);
  let scrubbing = false;
  let announced;

  const player = createPlayback(
    audio,
    (state) => {
      card.dataset.audioState = state.phase;
      const active = state.phase === "playing" || state.phase === "loading";
      const label = active ? "Pause" : state.phase === "ended" ? "Replay" : "Play";
      if (toggle) {
        toggle.setAttribute("aria-label", label);
        toggle.title = label;
      }
      if (status) status.textContent = AUDIO_STATUS[state.phase] || "Ready when you are";
      if (elapsed) elapsed.textContent = clock(state.position);
      if (total) total.textContent = clock(state.duration ?? NaN);
      if (seekBar instanceof HTMLInputElement) {
        seekBar.disabled = !state.seekable;
        if (!scrubbing) seekBar.value = String(Math.round(state.progress * 1000));
        seekBar.setAttribute(
          "aria-valuetext",
          clock(state.position) + " of " + clock(state.duration ?? NaN),
        );
      }
      for (const button of $$("[data-audio-skip]", card))
        /** @type {HTMLButtonElement} */ (button).disabled = !state.seekable;
      if (estimate)
        estimate.textContent =
          state.endsAt !== undefined
            ? "Ends at " + wallClock(state.endsAt)
            : state.remaining !== undefined
              ? clock(state.remaining) + " remaining"
              : "";
      if (mute) {
        const label = state.muted ? "Unmute" : "Mute";
        mute.setAttribute("aria-label", label);
        mute.setAttribute("aria-pressed", String(state.muted));
        mute.title = label;
      }
      if (volume instanceof HTMLInputElement && !volume.matches(":active")) {
        volume.hidden = !state.volumeAdjustable;
        volume.value = String(state.volume);
      }
      if (speed instanceof HTMLSelectElement) speed.value = String(state.speed);
      // Native controls come back if playback fails.
      audio.toggleAttribute("data-native-visible", state.phase === "error");
      if (state.duration !== announced) {
        announced = state.duration;
        if (state.duration) updateConsumption(article, "listen", state.duration);
      }
    },
    { duration: Number(audio.dataset.durationSeconds) },
  );

  toggle?.addEventListener("click", () => player.toggle());
  for (const button of $$("[data-audio-skip]", card))
    button.addEventListener("click", () => player.skip(Number(button.dataset.audioSkip)));
  seekBar?.addEventListener("input", (event) => {
    scrubbing = true;
    player.seek(Number(/** @type {HTMLInputElement} */ (event.currentTarget).value) / 1000);
  });
  seekBar?.addEventListener("change", (event) => {
    scrubbing = false;
    player.seek(Number(/** @type {HTMLInputElement} */ (event.currentTarget).value) / 1000);
  });
  seekBar?.addEventListener("blur", () => (scrubbing = false));
  speed?.addEventListener("change", (event) =>
    player.speed(Number(/** @type {HTMLSelectElement} */ (event.currentTarget).value)),
  );
  mute?.addEventListener("click", () => player.mute());
  volume?.addEventListener("input", (event) =>
    player.volume(Number(/** @type {HTMLInputElement} */ (event.currentTarget).value)),
  );

  controls.hidden = false;
  tools.hidden = false;
  card.classList.add("is-enhanced");
  window.addEventListener("pagehide", () => player.dispose());
}

/* ------------------------------------------------------------------ timing readout */

/** The "Ends at HH:MM" line that sits beside a player. */
function timingHost(player) {
  const sibling = player.nextElementSibling;
  const host = sibling?.matches("[data-media-timing]") ? sibling : null;
  const article = player.closest("article.item") || document;
  return (state) => {
    if (state.duration) updateConsumption(article, "watch", state.duration);
    const { endsAt: end } = endsAt(state);
    const text = end === undefined ? "" : "Ends at " + wallClock(end);
    if (host && host.textContent !== text) host.textContent = text;
  };
}

function enhanceNativeVideo(video) {
  const frame = video.closest(".native-video");
  if (!frame) return;
  const report = timingHost(frame);
  const archived = duration(Number(video.dataset.durationSeconds));
  let phase = video.paused ? "paused" : video.readyState >= 3 ? "playing" : "loading";
  const emit = () =>
    report({
      phase,
      duration: Number.isNaN(video.duration) ? archived : duration(video.duration),
      position: video.currentTime,
      speed: video.playbackRate,
    });
  const setPhase = (next) => {
    phase = next;
    emit();
  };
  for (const name of ["loadedmetadata", "durationchange", "timeupdate", "ratechange"])
    video.addEventListener(name, emit);
  video.addEventListener("play", () => setPhase("loading"));
  video.addEventListener("playing", () => setPhase("playing"));
  video.addEventListener("waiting", () => setPhase(video.paused ? "paused" : "loading"));
  video.addEventListener("pause", () => setPhase(video.ended ? "ended" : "paused"));
  video.addEventListener("ended", () => setPhase("ended"));
  video.addEventListener("error", () => setPhase("error"));
  emit();
}

/* ------------------------------------------------------------------ provider facades */

const VIMEO_EVENTS = [
  "play", "playing", "pause", "ended", "timeupdate", "playbackratechange",
  "bufferstart", "bufferend", "seeking", "seeked", "durationchange", "error",
];

const asRecord = (value) =>
  value && typeof value === "object" && !Array.isArray(value) ? value : undefined;

function messageData(value) {
  if (typeof value === "string") {
    if (value.length > 128000) return undefined;
    try {
      return asRecord(JSON.parse(value));
    } catch {
      return undefined;
    }
  }
  return asRecord(value);
}

const youtubePhase = (value) =>
  value === 1 ? "playing"
  : value === 2 ? "paused"
  : value === 3 ? "loading"
  : value === 0 ? "ended"
  : value === -1 || value === 5 ? "idle"
  : undefined;

/**
 * Follow a provider player's state over postMessage, so the reader gets a finish time without
 * downloading the provider's own SDK. Messages must match both the expected origin and window.
 */
function followProvider(frame, provider, origin, report, known) {
  let state = { phase: "idle", position: NaN, duration: duration(known), speed: NaN };
  let live = false;
  let recorded = Boolean(state.duration);
  let subscribed = false;
  let stale = false;
  let vimeoPlayback = "idle";
  let watchdog;

  const post = (message) => frame.contentWindow?.postMessage(message, origin);
  function emit() {
    clearTimeout(watchdog);
    report({ ...state });
    // A player that stops reporting is buffering, not still playing.
    if (state.phase === "playing" && state.duration)
      watchdog = setTimeout(() => {
        stale = true;
        state = { ...state, phase: "loading" };
        report({ ...state });
      }, 5000);
  }

  function subscribe() {
    if (provider === "youtube") {
      post(JSON.stringify({ event: "listening", id: "aggr-timing", channel: "widget" }));
      for (const event of ["onStateChange", "onPlaybackRateChange", "onError"])
        post(
          JSON.stringify({
            event: "command",
            func: "addEventListener",
            args: [event],
            id: "aggr-timing",
            channel: "widget",
          }),
        );
    } else {
      for (const event of VIMEO_EVENTS) post({ method: "addEventListener", value: event });
      for (const method of ["getDuration", "getCurrentTime", "getPlaybackRate", "getPaused"])
        post({ method });
    }
    subscribed = true;
  }

  window.addEventListener("message", (event) => {
    if (!frame.contentWindow || event.source !== frame.contentWindow || event.origin !== origin) return;
    const data = messageData(event.data);
    if (!data) return;

    if (provider === "youtube") {
      if (data.event === "readyToListen" || data.event === "onReady") return subscribe();
      if (data.event === "initialDelivery" || data.event === "infoDelivery") {
        const info = asRecord(data.info);
        if (!info) return;
        const video = asRecord(info.videoData);
        if (typeof video?.isLive === "boolean") {
          live = video.isLive;
          recorded = !live;
        }
        if (live) state.duration = undefined;
        else if (recorded && info.duration !== undefined) state.duration = duration(info.duration);
        if (typeof info.currentTime === "number" && Number.isFinite(info.currentTime) && info.currentTime >= 0) {
          state.position = info.currentTime;
          if (stale) state.phase = "playing";
          stale = false;
        }
        if (info.playbackRate !== undefined) state.speed = duration(info.playbackRate) ?? NaN;
        const phase = youtubePhase(info.playerState);
        if (phase) {
          state.phase = phase;
          stale = false;
        }
      } else if (data.event === "onStateChange") {
        state.phase = youtubePhase(data.info) ?? "idle";
        stale = false;
      } else if (data.event === "onPlaybackRateChange") state.speed = duration(data.info) ?? NaN;
      else if (data.event === "onError") {
        state.phase = "error";
        stale = false;
      } else return;
    } else {
      if (data.event === "ready") {
        if (!subscribed) subscribe();
        return;
      }
      const info = asRecord(data.data);
      if (data.method === "getDuration") state.duration = duration(data.value);
      else if (data.method === "getCurrentTime" && typeof data.value === "number" && data.value >= 0)
        state.position = data.value;
      else if (data.method === "getPlaybackRate") state.speed = duration(data.value) ?? NaN;
      else if (data.method === "getPaused") {
        if (data.value === true) {
          state.phase = vimeoPlayback = "paused";
          stale = false;
        }
      } else if (typeof data.event === "string" && VIMEO_EVENTS.includes(data.event)) {
        if (info?.duration !== undefined) state.duration = duration(info.duration);
        if (typeof info?.seconds === "number" && Number.isFinite(info.seconds) && info.seconds >= 0) {
          state.position = info.seconds;
          if (data.event === "timeupdate" && stale) {
            state.phase = vimeoPlayback;
            stale = false;
          }
        }
        if (!["timeupdate", "playbackratechange", "durationchange"].includes(data.event)) stale = false;
        if (info?.playbackRate !== undefined) state.speed = duration(info.playbackRate) ?? NaN;
        if (data.event === "playing") state.phase = vimeoPlayback = "playing";
        else if (["play", "bufferstart", "seeking"].includes(data.event)) state.phase = "loading";
        else if (["seeked", "bufferend"].includes(data.event)) state.phase = vimeoPlayback;
        else if (data.event === "pause") state.phase = vimeoPlayback = "paused";
        else if (data.event === "ended") state.phase = vimeoPlayback = "ended";
        else if (data.event === "error") state.phase = vimeoPlayback = "error";
      } else return;
    }
    emit();
  });

  return subscribe;
}

/** Nothing reaches the provider until the reader activates the poster. */
function enhanceVideoFacade(preview) {
  const player = preview.closest(".video-player");
  if (!player || player.dataset.videoBound === "true") return;
  const provider = player.dataset.videoProvider;
  // Twitch refuses to embed outside a secure context.
  if (
    provider === "twitch" &&
    location.protocol !== "https:" &&
    !["localhost", "127.0.0.1", "[::1]"].includes(location.hostname)
  )
    return;
  player.dataset.videoBound = "true";
  const title = preview.getAttribute("aria-label") || "Video";

  function activate(event) {
    if (event.type === "keydown" && event.key !== " " && event.key !== "Enter") return;
    event.preventDefault();
    if (player.dataset.videoMounted === "true") return;
    player.dataset.videoMounted = "true";

    const url = new URL(preview.dataset.videoEmbed || "");
    if (preview.hasAttribute("data-video-parent")) url.searchParams.set("parent", location.hostname);
    url.searchParams.set("autoplay", provider === "twitch" ? "true" : "1");
    if (provider === "youtube") {
      url.searchParams.set("enablejsapi", "1");
      url.searchParams.set("origin", location.origin);
    }

    const frame = document.createElement("iframe");
    frame.src = url.href;
    frame.title = title;
    frame.loading = "eager";
    frame.allow = "autoplay; encrypted-media; fullscreen; picture-in-picture";
    frame.allowFullscreen = true;
    frame.referrerPolicy = "strict-origin-when-cross-origin";
    frame.setAttribute("sandbox", "allow-scripts allow-same-origin allow-presentation");

    const report = timingHost(player);
    const subscribe =
      provider === "youtube" || provider === "vimeo"
        ? followProvider(frame, provider, url.origin, report, Number(player.dataset.durationSeconds))
        : null;

    player.classList.add("is-loading");
    player.setAttribute("aria-busy", "true");
    frame.addEventListener(
      "load",
      () => {
        player.classList.add("is-loaded");
        player.removeAttribute("aria-busy");
        preview.hidden = true;
        subscribe?.();
        // `autoplay` alone leaves YouTube and Vimeo showing their own play button when the
        // browser declines; the activating click already authorized this, so ask the player too.
        const target = frame.contentWindow;
        if (!target) return;
        if (provider === "youtube") {
          target.postMessage(
            JSON.stringify({ event: "listening", id: "aggr-play", channel: "widget" }),
            url.origin,
          );
          target.postMessage(
            JSON.stringify({
              event: "command",
              func: "playVideo",
              args: [],
              id: "aggr-play",
              channel: "widget",
            }),
            url.origin,
          );
        } else if (provider === "vimeo") target.postMessage({ method: "play" }, url.origin);
      },
      { once: true },
    );
    player.appendChild(frame);

    // Twitch refuses to render below 400×300. A narrow column gets a player laid out at that
    // size and scaled down to fit, rather than a player that widens the article or a refusal.
    if (provider === "twitch") {
      const fit = (width, height) => {
        const scale = width && width < 400 ? width / 400 : 1;
        frame.style.width = scale < 1 ? "400px" : "100%";
        frame.style.height =
          scale < 1 ? Math.max(300, Math.ceil((400 * height) / width)) + "px" : "100%";
        frame.style.transform = scale < 1 ? "scale(" + scale + ")" : "";
        frame.style.transformOrigin = "top left";
      };
      const box = player.getBoundingClientRect();
      fit(box.width, box.height);
      if ("ResizeObserver" in window) {
        new ResizeObserver((entries) => {
          for (const entry of entries) fit(entry.contentRect.width, entry.contentRect.height);
        }).observe(player);
      }
    }
  }

  preview.addEventListener("click", activate);
  preview.addEventListener("keydown", activate);
}

export function mount() {
  for (const card of $$("[data-audio-component]")) enhanceAudio(card);
  for (const video of $$(".native-video video")) enhanceNativeVideo(video);
  for (const preview of $$("[data-video-embed]")) enhanceVideoFacade(preview);
}
