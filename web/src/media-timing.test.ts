import { expect, it, vi } from "vitest";
import { playbackTiming, formatPlaybackEnd, createNativeTiming, createProviderTiming, type NativeTimingPort } from "./media-timing";

it("computes remaining wall time at the actual playback speed and suppresses inactive/live estimates", () => {
  const playing = { phase: "playing", position: 60, duration: 180, speed: 2 };
  expect(playbackTiming(playing, 1_000)).toEqual({ remainingSeconds: 60, endsAt: 61_000 });
  for (const phase of ["idle", "paused", "loading", "ended", "error"]) expect(playbackTiming({ ...playing, phase }, 1_000).endsAt).toBeUndefined();
  for (const duration of [undefined, Infinity, NaN, 0, -1]) expect(playbackTiming({ ...playing, duration }, 1_000)).toEqual({});
  for (const speed of [0, -1, Infinity, NaN]) expect(playbackTiming({ ...playing, speed }, 1_000)).toEqual({});
  expect(playbackTiming({ ...playing, position: 200 }, 1_000).endsAt).toBeUndefined();
  expect(formatPlaybackEnd(new Date(2026, 8, 9, 7, 5).getTime())).toBe("07:05");
});

class NativeVideo extends EventTarget implements NativeTimingPort {
  duration = NaN; currentTime = 0; playbackRate = 1; paused = true; ended = false; readyState = 0;
  pause = vi.fn(() => { this.paused = true; });
}
it("uses archived native duration before metadata, clears buffering/live estimates and releases listeners", () => {
  const video = new NativeVideo(), update = vi.fn();
  const timing = createNativeTiming(video, update, 120);
  expect(update.mock.lastCall?.[0]).toMatchObject({ duration: 120, phase: "paused" });
  video.duration = 150; video.currentTime = 30; video.paused = false; video.readyState = 4;
  video.dispatchEvent(new Event("playing"));
  expect(update.mock.lastCall?.[0]).toMatchObject({ duration: 150, position: 30, phase: "playing" });
  video.dispatchEvent(new Event("waiting"));
  expect(update.mock.lastCall?.[0].phase).toBe("loading");
  video.duration = Infinity; video.dispatchEvent(new Event("durationchange"));
  expect(update.mock.lastCall?.[0].duration).toBeUndefined();
  timing.dispose(); const calls = update.mock.calls.length;
  video.dispatchEvent(new Event("playing"));
  expect(update).toHaveBeenCalledTimes(calls); expect(video.pause).toHaveBeenCalledOnce();
});

function provider(kind: "youtube" | "vimeo") {
  const source = {}, update = vi.fn(), post = vi.fn();
  const origin = kind === "youtube" ? "https://www.youtube-nocookie.com" : "https://player.vimeo.com";
  const timing = createProviderTiming({ provider: kind, origin, source: () => source, post, update });
  const send = (data: unknown, overrides = {}) => timing.message({ origin, source, data, ...overrides });
  return { timing, send, update, source, origin, post };
}
it("accepts YouTube telemetry only from the exact iframe and origin, and distinguishes live broadcasts", () => {
  const { timing, send, update } = provider("youtube");
  const info = { event: "infoDelivery", info: { duration: 180, currentTime: 60, playerState: 1, playbackRate: 2, videoData: { isLive: false } } };
  send(info, { origin: "https://www.youtube-nocookie.com.evil.test" }); send(info, { source: {} });
  expect(update).not.toHaveBeenCalled();
  send(JSON.stringify(info)); expect(update.mock.lastCall?.[0]).toMatchObject({ phase: "playing", duration: 180, position: 60, speed: 2 });
  send({ event: "onStateChange", info: 2 }); expect(update.mock.lastCall?.[0].phase).toBe("paused");
  send({ event: "infoDelivery", info: { videoData: { isLive: true }, playerState: 1, duration: 4000 } });
  expect(update.mock.lastCall?.[0].duration).toBeUndefined();
  timing.dispose(); const calls = update.mock.calls.length; send(info);
  expect(update).toHaveBeenCalledTimes(calls);
});
it("does not treat YouTube elapsed live time or malformed data as a finite recording", () => {
  const { send, update } = provider("youtube");
  send({ event: "infoDelivery", info: { duration: 500, currentTime: 20, playerState: 1 } });
  expect(update.mock.lastCall?.[0].duration).toBeUndefined();
  send("invalid JSON"); send(null); send({ event: "unrelated", info: {} });
  expect(update).toHaveBeenCalledOnce();
});
it("subscribes Vimeo only when loaded and tracks real playing, buffering, rate and pause events", () => {
  const { timing, send, update, post } = provider("vimeo");
  expect(post).not.toHaveBeenCalled(); timing.loaded();
  expect(post).toHaveBeenCalledWith({ method: "addEventListener", value: "timeupdate" });
  send({ event: "play", data: { duration: 120, seconds: 0 } }); expect(update.mock.lastCall?.[0].phase).toBe("loading");
  send({ event: "playing", data: { duration: 120, seconds: 10 } });
  send({ event: "playbackratechange", data: { playbackRate: 1.5 } });
  expect(update.mock.lastCall?.[0]).toMatchObject({ phase: "playing", speed: 1.5, position: 10, duration: 120 });
  send({ event: "bufferstart" }); expect(update.mock.lastCall?.[0].phase).toBe("loading");
  send({ event: "pause", data: { duration: 120, seconds: 12 } }); expect(update.mock.lastCall?.[0].phase).toBe("paused");
  timing.dispose(); const calls = post.mock.calls.length; timing.loaded();
  expect(post).toHaveBeenCalledTimes(calls);
});

it("withdraws stale provider playback timing and cancels its bounded watchdog on disposal", () => {
  vi.useFakeTimers();
  try {
    const { timing, send, update } = provider("youtube");
    const sample = { event: "infoDelivery", info: { duration: 180, currentTime: 20, playerState: 1, playbackRate: 1, videoData: { isLive: false } } };
    send(sample);
    expect(vi.getTimerCount()).toBe(1);
    vi.advanceTimersByTime(5000);
    expect(update.mock.lastCall?.[0].phase).toBe("loading");
    send({ event: "infoDelivery", info: { currentTime: 30 } });
    expect(update.mock.lastCall?.[0]).toMatchObject({ phase: "playing", position: 30 });
    send(sample); timing.dispose();
    expect(vi.getTimerCount()).toBe(0);
  } finally { vi.useRealTimers(); }
});

it("resumes verified Vimeo playback after seeking/buffering without reviving paused playback", () => {
  const { send, update, timing } = provider("vimeo");
  send({ event: "playing", data: { duration: 120, seconds: 10 } });
  send({ event: "seeking", data: { duration: 120, seconds: 60 } });
  expect(update.mock.lastCall?.[0].phase).toBe("loading");
  send({ event: "seeked", data: { duration: 120, seconds: 60 } });
  expect(update.mock.lastCall?.[0]).toMatchObject({ phase: "playing", position: 60 });
  send({ event: "bufferstart" }); send({ event: "bufferend" });
  expect(update.mock.lastCall?.[0].phase).toBe("playing");
  send({ event: "bufferstart" }); send({ event: "pause" }); send({ event: "bufferend" });
  expect(update.mock.lastCall?.[0].phase).toBe("paused");
  send({ event: "seeking" }); send({ event: "seeked" });
  expect(update.mock.lastCall?.[0].phase).toBe("paused");
  timing.dispose();
});
