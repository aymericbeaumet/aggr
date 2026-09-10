import { afterEach, describe, expect, it, vi } from "vitest";
import { createPlayback, formatAudioTime, type AudioPort, type PlaybackSnapshot } from "./audio";

class TestAudio extends EventTarget implements AudioPort {
  currentTime = 0;
  duration = NaN;
  paused = true;
  ended = false;
  playbackRate = 1;
  volume = 1;
  muted = false;
  error = null;
  play = vi.fn(async () => { this.paused = false; this.dispatchEvent(new Event("playing")); });
  pause = vi.fn(() => { this.paused = true; this.dispatchEvent(new Event("pause")); });
  load = vi.fn();
}

afterEach(()=>{vi.useRealTimers();});

describe("audio playback", () => {
  it("waits for duration and clamps seeking and skips to the recording", () => {
    const audio = new TestAudio();
    const states: PlaybackSnapshot[] = [];
    const player = createPlayback(audio, state => states.push(state));
    player.seek(0.8);
    expect(audio.currentTime).toBe(0);
    expect(states.at(-1)?.duration).toBeUndefined();
    audio.duration = 120;
    audio.dispatchEvent(new Event("loadedmetadata"));
    player.skip(30);
    expect(audio.currentTime).toBe(30);
    player.seek(0.75);
    expect(audio.currentTime).toBe(90);
    player.skip(300);
    expect(audio.currentTime).toBe(120);
    player.skip(-300);
    expect(audio.currentTime).toBe(0);
    player.speed(1.5);
    expect(audio.playbackRate).toBe(1.5);
    player.speed(99);
    expect(audio.playbackRate).toBe(1.5);
    player.speed(.5);expect(audio.playbackRate).toBe(.5);
    player.speed(3);expect(audio.playbackRate).toBe(3);
    player.dispose();
  });

  it("cancels a pending play without turning its late rejection into an error", async () => {
    const audio = new TestAudio();
    let rejectPlay: ((reason: Error) => void) | undefined;
    audio.play.mockImplementation(() => new Promise((_resolve, reject) => { rejectPlay = reject; }));
    const states: PlaybackSnapshot[] = [];
    const player = createPlayback(audio, state => states.push(state));
    player.toggle();
    expect(states.at(-1)?.phase).toBe("loading");
    player.toggle();
    rejectPlay?.(new Error("play interrupted"));
    await Promise.resolve();
    expect(states.at(-1)?.phase).toBe("paused");
    player.dispose();
  });

  it("disposes listeners and pending playback when navigation removes the player", async () => {
    const audio = new TestAudio();
    let rejectPlay: ((reason: Error) => void) | undefined;
    audio.play.mockImplementation(() => new Promise((_resolve, reject) => { rejectPlay = reject; }));
    const update = vi.fn();
    const player = createPlayback(audio, update);
    player.toggle();
    player.dispose();
    const calls = update.mock.calls.length;
    rejectPlay?.(new Error("late network failure"));
    audio.dispatchEvent(new Event("error"));
    await Promise.resolve();
    expect(update).toHaveBeenCalledTimes(calls);
    expect(audio.paused).toBe(true);
  });

  it("reports playback errors and supports an explicit retry", async () => {
    const audio = new TestAudio();
    audio.play.mockRejectedValueOnce(new Error("network"));
    const states: PlaybackSnapshot[] = [];
    const player = createPlayback(audio, state => states.push(state));
    player.toggle();
    await Promise.resolve();
    expect(states.at(-1)?.phase).toBe("error");
    player.toggle();
    await Promise.resolve();
    expect(states.at(-1)?.phase).toBe("playing");
    player.dispose();
  });

  it("formats long episodes and unavailable durations without invalid numbers", () => {
    expect(formatAudioTime(3661.9)).toBe("1:01:01");
    expect(formatAudioTime(65)).toBe("1:05");
    expect(formatAudioTime(0)).toBe("0:00");
    expect(formatAudioTime(Infinity)).toBe("—");
    expect(formatAudioTime(NaN)).toBe("—");
  });

  it("uses a known episode length for display but waits for metadata before seeking",()=>{
    const audio=new TestAudio();let state!:PlaybackSnapshot;
    const player=createPlayback(audio,next=>{state=next;},{duration:3600});
    expect(state.duration).toBe(3600);expect(state.seekable).toBe(false);
    player.seek(.5);expect(audio.currentTime).toBe(0);
    audio.duration=3500;audio.dispatchEvent(new Event('loadedmetadata'));
    expect(state.duration).toBe(3500);expect(state.seekable).toBe(true);
    player.seek(.5);expect(audio.currentTime).toBe(1750);
    player.dispose();
  });

  it("predicts the finish from position, rate and clock only while actually playing",()=>{
    vi.useFakeTimers();vi.setSystemTime(1_000_000);
    const audio=new TestAudio();audio.duration=3600;audio.currentTime=1200;
    let state!:PlaybackSnapshot;const player=createPlayback(audio,next=>{state=next;});
    expect(state.endsAt).toBeUndefined();expect(state.remainingSeconds).toBe(2400);
    player.toggle();expect(state.endsAt).toBe(3_400_000);
    player.speed(2);expect(state.endsAt).toBe(2_200_000);
    player.seek(.5);expect(state.endsAt).toBe(1_900_000);
    audio.dispatchEvent(new Event('waiting'));expect(state.endsAt).toBeUndefined();
    audio.dispatchEvent(new Event('playing'));expect(state.endsAt).toBe(1_900_000);
    player.toggle();expect(state.endsAt).toBeUndefined();
    player.dispose();
  });

  it("clamps volume, restores a nonzero level on unmute, and follows native volume changes",()=>{
    const audio=new TestAudio();let state!:PlaybackSnapshot;
    const player=createPlayback(audio,next=>{state=next;});
    player.volume(.4);expect(audio.volume).toBe(.4);
    player.mute();expect(state.muted).toBe(true);
    player.volume(.6);expect(state.muted).toBe(false);expect(state.volume).toBe(.6);
    player.volume(0);player.mute();expect(state.volume).toBe(.6);expect(state.muted).toBe(false);
    player.volume(2);expect(state.volume).toBe(1);
    player.volume(NaN);expect(state.volume).toBe(1);
    audio.muted=true;audio.dispatchEvent(new Event('volumechange'));expect(state.muted).toBe(true);
    player.dispose();player.volume(.5);player.mute();expect(audio.volume).toBe(1);expect(audio.muted).toBe(true);
  });

  it("stops offering a volume slider when the platform rejects software volume changes",()=>{
    const audio=new TestAudio();Object.defineProperty(audio,'volume',{get:()=>1,set:()=>{}});
    let state!:PlaybackSnapshot;const player=createPlayback(audio,next=>{state=next;});
    player.volume(.5);expect(state.volumeAdjustable).toBe(false);expect(state.volume).toBe(1);
    player.mute();expect(state.muted).toBe(true);
    player.dispose();
  });

});
