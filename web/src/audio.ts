import { mount, unmount } from "svelte";
import { writable } from "svelte/store";
import AudioControls from "./components/AudioControls.svelte";
import { playbackTiming, updateConsumption } from "./media-timing";

export interface AudioPort extends EventTarget {
  currentTime: number;
  duration: number;
  paused: boolean;
  ended: boolean;
  playbackRate: number;
  volume: number;
  muted: boolean;
  error: { code: number } | null;
  play(): Promise<void>;
  pause(): void;
  load(): void;
}

export interface PlaybackSnapshot {
  phase: "idle" | "loading" | "playing" | "paused" | "ended" | "error";
  position: number;
  duration?: number;
  progress: number;
  speed: number;
  seekable: boolean;
  volume: number;
  muted: boolean;
  volumeAdjustable: boolean;
  remainingSeconds?: number;
  endsAt?: number;
}

export const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2, 2.5, 3];

export function formatAudioTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "—";
  const whole = Math.floor(seconds);
  const hours = Math.floor(whole / 3600);
  const minutes = Math.floor(whole / 60) % 60;
  return hours ? `${hours}:${String(minutes).padStart(2, "0")}:${String(whole % 60).padStart(2, "0")}`
    : `${minutes}:${String(whole % 60).padStart(2, "0")}`;
}

export function createPlayback(audio: AudioPort, update: (state: PlaybackSnapshot) => void, known: {duration?:number} = {}) {
  const listeners = new AbortController();
  let phase: PlaybackSnapshot["phase"] = audio.paused ? "idle" : "playing";
  let attempt = 0;
  let disposed = false;
  let volumeAdjustable = true;
  let previousVolume = audio.volume > 0 ? audio.volume : 1;

  function seekableDuration() { return Number.isFinite(audio.duration) && audio.duration > 0 ? audio.duration : undefined; }
  function duration() {
    if (audio.duration === Infinity) return;
    return seekableDuration() ?? (known.duration && Number.isFinite(known.duration) && known.duration > 0 ? known.duration : undefined);
  }
  function emit() {
    if (disposed) return;
    const length = duration();
    const position = Number.isFinite(audio.currentTime) ? Math.max(0, Math.min(audio.currentTime, length ?? Infinity)) : 0;
    if(audio.volume > 0) previousVolume=audio.volume;
    const state={phase,position,duration:length,speed:audio.playbackRate};
    update({...state,progress:length ? position/length : 0,seekable:!!seekableDuration(),
      volume:audio.volume,muted:audio.muted || audio.volume === 0,volumeAdjustable,...playbackTiming(state)});
  }
  function setPhase(next: PlaybackSnapshot["phase"]) { phase = next; emit(); }
  for (const event of ["loadedmetadata", "durationchange", "timeupdate", "ratechange", "volumechange"]) {
    audio.addEventListener(event, emit, { signal: listeners.signal });
  }
  audio.addEventListener("play", () => setPhase("loading"), { signal: listeners.signal });
  audio.addEventListener("playing", () => setPhase("playing"), { signal: listeners.signal });
  audio.addEventListener("waiting", () => { if (!audio.paused) setPhase("loading"); }, { signal: listeners.signal });
  audio.addEventListener("pause", () => setPhase(audio.ended ? "ended" : "paused"), { signal: listeners.signal });
  audio.addEventListener("ended", () => setPhase("ended"), { signal: listeners.signal });
  audio.addEventListener("error", () => setPhase("error"), { signal: listeners.signal });

  function toggle() {
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
  }
  function seek(fraction: number) {
    const length = seekableDuration();
    if (disposed || !length || !Number.isFinite(fraction)) return;
    try { audio.currentTime = Math.max(0, Math.min(1, fraction)) * length; emit(); } catch { /* Not seekable yet. */ }
  }
  function skip(seconds: number) {
    const length = seekableDuration();
    if (length && Number.isFinite(seconds)) seek((audio.currentTime + seconds) / length);
  }
  function speed(value: number) {
    if (disposed || !SPEEDS.includes(value)) return;
    try { audio.playbackRate = value; emit(); } catch { /* The browser may restrict playback rates. */ }
  }
  function volume(value:number) {
    if(disposed || !Number.isFinite(value)) return;
    const next=Math.max(0,Math.min(1,value));
    try {
      audio.volume=next;
      volumeAdjustable=Math.abs(audio.volume-next) < .001;
      if(next > 0 && volumeAdjustable) audio.muted=false;
    } catch {volumeAdjustable=false;}
    emit();
  }
  function mute() {
    if(disposed) return;
    try {
      if(audio.muted || audio.volume === 0) {
        if(audio.volume === 0) volume(previousVolume);
        audio.muted=false;
      } else audio.muted=true;
    } catch { /* Some platforms delegate volume entirely to device controls. */ }
    emit();
  }
  function dispose() {
    if(disposed) return;
    disposed = true;
    attempt += 1;
    listeners.abort();
    audio.pause();
  }
  emit();
  return { toggle, seek, skip, speed, volume, mute, dispose };
}

export type PlaybackController = ReturnType<typeof createPlayback>;

function mountAudio(card: HTMLElement, audio: HTMLAudioElement) {
  const original = Array.from(card.childNodes);
  const artwork = card.querySelector<HTMLImageElement>(".audio-artwork")?.getAttribute("src") ?? undefined;
  const heading = card.querySelector(".audio-heading strong")?.textContent || "Listen";
  const article = card.closest("article.item");
  let consumptionDuration: number | undefined;
  const model = writable<PlaybackSnapshot>();
  const player = createPlayback(audio, state => {
    card.dataset.audioState = state.phase;
    model.set(state);
    if(state.duration !== consumptionDuration) {
      consumptionDuration=state.duration;
      if(article && state.duration) updateConsumption(article,"listen",state.duration);
    }
  },{duration:Number(audio.dataset.durationSeconds)});
  card.replaceChildren();
  let component: ReturnType<typeof mount>;
  try {component = mount(AudioControls, { target: card, props: { model, player, audio, artwork, heading } });}
  catch {
    player.dispose();
    card.replaceChildren(...original);
    delete card.dataset.audioState;
    return async () => {};
  }
  card.classList.add("is-enhanced");
  return async () => {
    player.dispose();
    await unmount(component);
    const source = audio.getAttribute("src");
    audio.removeAttribute("src");
    audio.load();
    if (source) audio.setAttribute("src", source);
    audio.removeAttribute("data-native-visible");
    card.classList.remove("is-enhanced");
    delete card.dataset.audioState;
    card.replaceChildren(...original);
  };
}

export function createAudio() {
  const players = new Map<HTMLAudioElement, () => Promise<void>>();

  function enhance(root: ParentNode = document) {
    for (const card of root.querySelectorAll<HTMLElement>("[data-audio-component]")) {
      const audio = card.querySelector("audio");
      if (!audio || players.has(audio)) continue;
      players.set(audio, mountAudio(card, audio));
    }
  }
  async function dispose() {
    const releases = [...players.values()];
    players.clear();
    await Promise.all(releases.map(release => release()));
  }
  return { enhance, dispose };
}
