<script lang="ts">
  // The now-playing bar. Persistent chrome, outside the page key: started media keeps
  // playing while the reader moves, and this bar follows it until that player is on screen.
  // A picture rides in the bar; audio stays clipped and the bar shows its artwork.
  // Idle, including on the server, it renders nothing, so the static page is unchanged.
  import { page } from '../state/page.svelte';
  import { clock } from '../media/timing';
  import {
    cardInView,
    cycleSpeed,
    dismiss,
    mutePlayback,
    placeVisual,
    seekPlayback,
    skipPlayback,
    subscribe,
    togglePlayback,
    volumePlayback,
    watchOnScreen,
    type Session,
  } from '../media/now-playing';

  let session = $state<Session | null>(null);
  let onScreen = $state(false);
  // Separate from `session`: the picture effect must not re-run on every elapsed-time tick.
  // Re-running it cancels the glide and starts another, which reads as the player reloading.
  let visual = $state(false);
  let bar = $state<HTMLElement | null>(null);
  let stage = $state<HTMLElement | null>(null);
  let seek = $state<HTMLInputElement | null>(null);
  let level = $state<HTMLInputElement | null>(null);
  let scrubbing = false;
  let leveling = false;

  $effect(() => {
    if (typeof document === 'undefined') return;
    return subscribe((next) => {
      session = next;
      visual = Boolean(next?.visual);
      onScreen = next ? cardInView(next.episode.src) : false;
    });
  });

  $effect(() => {
    if (typeof document === 'undefined') return;
    const src = session?.episode.src;
    // Navigation replaces the card after this bar has already subscribed. Look again.
    void page.href;
    if (!src) return;
    return watchOnScreen(src, (visible) => {
      onScreen = visible;
    });
  });

  $effect(() => {
    if (typeof document === 'undefined') return;
    const root = document.documentElement;
    const shown = Boolean(session) && !onScreen;
    if (!shown) {
      root.removeAttribute('data-now-playing');
      root.style.removeProperty('--now-playing-offset');
      return;
    }
    root.setAttribute('data-now-playing', '');
    const node = bar;
    if (!node) return;
    const apply = () => root.style.setProperty('--now-playing-offset', `${node.offsetHeight}px`);
    apply();
    if (typeof ResizeObserver !== 'function') return;
    const observer = new ResizeObserver(apply);
    observer.observe(node);
    return () => observer.disconnect();
  });

  // The stage is only the landing box. The picture itself stays in its seat and scales onto it.
  // Depends on `visual` and `stage`, not on the session: a clock tick must not restart the glide.
  $effect(() => {
    if (typeof document === 'undefined' || !visual) return;
    const node = stage;
    if (!node) return;
    return placeVisual(node);
  });

  $effect(() => {
    const progress = session?.state.progress;
    if (seek && progress !== undefined && !scrubbing) seek.value = String(Math.round(progress * 1000));
  });

  $effect(() => {
    const volume = session?.state.volume;
    if (level && volume !== undefined && !leveling) level.value = String(volume);
  });

  const label = $derived.by(() => {
    const phase = session?.state.phase;
    if (phase === 'playing' || phase === 'loading') return 'Pause';
    if (phase === 'ended') return 'Replay';
    return 'Play';
  });

  function rate(value: number): string {
    return Number.isFinite(value) ? `${value}×` : '1×';
  }

  function onSeek(event: Event, done: boolean): void {
    // Events are delegated, so the range is `target`, not `currentTarget`.
    const input = event.target;
    if (!(input instanceof HTMLInputElement)) return;
    scrubbing = !done;
    seekPlayback(Number(input.value) / 1000);
  }

  function onVolume(event: Event, done: boolean): void {
    const input = event.target;
    if (!(input instanceof HTMLInputElement)) return;
    leveling = !done;
    volumePlayback(Number(input.value));
  }
</script>

{#if session && !onScreen}
<section class="now-playing" data-audio-state={session.state.phase} data-visual={session.visual ? '' : undefined} aria-label="Now playing" bind:this={bar}>
  {#if session.visual}
  <div class="now-playing-stage" bind:this={stage}></div>
  {/if}
  <input
    class="now-playing-seek"
    type="range"
    min="0"
    max="1000"
    step="1"
    bind:this={seek}
    disabled={!session.controls || !session.state.seekable}
    aria-label="Playback position"
    aria-valuetext="{clock(session.state.position)} of {clock(session.state.duration ?? NaN)}"
    oninput={(event) => onSeek(event, false)}
    onchange={(event) => onSeek(event, true)}
    onblur={() => (scrubbing = false)}
  >
  <a class="now-playing-episode" href={session.episode.page}>
    {#if session.episode.artwork}
    <img class="now-playing-art" src={session.episode.artwork} alt="" width="44" height="44">
    {:else}
    <span class="now-playing-mark" aria-hidden="true"></span>
    {/if}
    <span class="now-playing-titles">
      {#if session.episode.show}<span class="now-playing-show">{session.episode.show}</span>{/if}
      <span class="now-playing-title">{session.episode.title}</span>
      {#if session.state.phase === 'error'}<span class="now-playing-status" role="status">{session.visual ? 'Video unavailable. Try playing again.' : 'Audio unavailable. Try playing again.'}</span>{/if}
    </span>
  </a>
  <div class="now-playing-transport">
    <button class="audio-skip" type="button" aria-label="Back 15 seconds" title="Back 15 seconds" disabled={!session.controls || !session.state.seekable} onclick={() => skipPlayback(-15)}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 4.5a8 8 0 1 1-6.93 4"/><path d="M14.5 2 12 4.5 14.5 7"/></svg><span aria-hidden="true">15</span>
    </button>
    <button class="audio-toggle" type="button" aria-label={label} title={label} disabled={!session.controls} onclick={() => togglePlayback()}>
      <svg class="audio-play-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="m8 5 11 7-11 7z"/></svg>
      <svg class="audio-pause-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M6 5h4v14H6zm8 0h4v14h-4z"/></svg>
    </button>
    <button class="audio-skip" type="button" aria-label="Forward 30 seconds" title="Forward 30 seconds" disabled={!session.controls || !session.state.seekable} onclick={() => skipPlayback(30)}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 4.5a8 8 0 1 0 6.93 4"/><path d="M9.5 2 12 4.5 9.5 7"/></svg><span aria-hidden="true">30</span>
    </button>
    {#if !session.visual}
    <div class="now-playing-volume">
      <button class="now-playing-mute" type="button" aria-label={session.state.muted ? 'Unmute' : 'Mute'} aria-pressed={session.state.muted} title={session.state.muted ? 'Unmute' : 'Mute'} onclick={() => mutePlayback()}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M11 5 6 9H3v6h3l5 4z"/><path class="audio-volume-wave" d="M15 8a6 6 0 0 1 0 8m3-11a10 10 0 0 1 0 14"/></svg>
      </button>
      {#if session.state.volumeAdjustable}
      <input
        class="now-playing-level"
        type="range"
        min="0"
        max="1"
        step="0.05"
        value="1"
        bind:this={level}
        aria-label="Volume"
        aria-valuetext="{Math.round(session.state.volume * 100)}%"
        oninput={(event) => onVolume(event, false)}
        onchange={(event) => onVolume(event, true)}
        onblur={() => (leveling = false)}
      >
      {/if}
    </div>
    {/if}
  </div>
  <span class="now-playing-time">{clock(session.state.position)}</span>
  <button class="now-playing-speed" type="button" aria-label="Playback speed, {rate(session.state.speed)}" disabled={!session.controls} onclick={() => cycleSpeed()}>{rate(session.state.speed)}</button>
  <button class="now-playing-close" type="button" aria-label="Stop and close" onclick={() => dismiss()}>×</button>
</section>
{/if}
