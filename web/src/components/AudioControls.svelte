<script lang="ts">
  import type { Readable } from 'svelte/store';
  import { formatAudioTime, SPEEDS, type PlaybackController, type PlaybackSnapshot } from '../audio';
  import { formatPlaybackEnd } from '../media-timing';

  let { model, player, audio, artwork, heading }: {
    model: Readable<PlaybackSnapshot>; player: PlaybackController; audio: HTMLAudioElement;
    artwork?: string; heading: string;
  } = $props();
  let scrubbing = $state(false);
  let progress = $state(0);
  let coverImage: HTMLImageElement | undefined = $state();
  let coverLoaded = $state(false);
  $effect(() => { artwork; coverLoaded = !!coverImage?.complete && coverImage.naturalWidth > 0; });
  const active = $derived($model.phase === 'playing' || $model.phase === 'loading');
  const label = $derived(active ? 'Pause' : $model.phase === 'ended' ? 'Replay' : 'Play');
  const status = $derived($model.phase === 'error' ? 'Audio unavailable. Try playing again.'
    : $model.phase === 'loading' ? 'Loading audio…' : $model.phase === 'playing' ? 'Playing'
    : $model.phase === 'paused' ? 'Paused' : $model.phase === 'ended' ? 'Finished' : 'Ready when you are');
  $effect(() => { if (!scrubbing) progress = Math.round($model.progress * 1000); });
  const estimate = $derived($model.endsAt !== undefined ? `Ends at ${formatPlaybackEnd($model.endsAt)}`
    : $model.remainingSeconds !== undefined ? `${formatAudioTime($model.remainingSeconds)} remaining` : '');
  $effect(() => { audio.toggleAttribute('data-native-visible', $model.phase === 'error'); });

  function nativeHost(node: HTMLElement) {
    node.append(audio);
    return { destroy() { if (audio.parentNode === node) audio.remove(); } };
  }
  function seek(event: Event, dragging: boolean) {
    scrubbing = dragging;
    progress = Number((event.currentTarget as HTMLInputElement).value);
    player.seek(progress / 1000);
  }
</script>

{#if artwork}<div class="audio-cover" class:is-loaded={coverLoaded}><img bind:this={coverImage} class="audio-artwork" src={artwork} alt="" loading="lazy" decoding="async" onload={() => { coverLoaded = true; }} /></div>{/if}
<div class="audio-panel">
  <div class="audio-heading">
    <strong>{heading}</strong>
    <p class="audio-status" data-audio-status role="status" aria-live="polite">{status}</p>
    <p class="audio-estimate" data-audio-ends-at>{estimate}</p>
  </div>
  <div use:nativeHost style="display: contents"></div>
  <div class="audio-controls" data-audio-controls>
    <div class="audio-transport">
      <button class="audio-skip" type="button" data-audio-back aria-label="Back 15 seconds" title="Back 15 seconds" disabled={!$model.seekable} onclick={() => player.skip(-15)}>↶ 15</button>
      <button class="audio-toggle" type="button" data-audio-toggle aria-label={label} title={label} onclick={player.toggle}>
        <svg class="audio-play-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="m8 5 11 7-11 7z" /></svg>
        <svg class="audio-pause-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M6 5h4v14H6zm8 0h4v14h-4z" /></svg>
      </button>
      <button class="audio-skip" type="button" data-audio-forward aria-label="Forward 30 seconds" title="Forward 30 seconds" disabled={!$model.seekable} onclick={() => player.skip(30)}>30 ↷</button>
    </div>
    <div class="audio-timeline">
      <input type="range" min="0" max="1000" value={progress} step="1" data-audio-seek aria-label="Playback position"
        disabled={!$model.seekable} aria-valuetext={`${formatAudioTime($model.position)} of ${formatAudioTime($model.duration ?? NaN)}`}
        oninput={event => seek(event, true)} onchange={event => seek(event, false)} onblur={() => { scrubbing = false; }} />
      <div class="audio-times"><span data-audio-elapsed>{formatAudioTime($model.position)}</span><span data-audio-duration>{formatAudioTime($model.duration ?? NaN)}</span></div>
    </div>
  </div>
  <div class="audio-tools" data-audio-tools>
    <label class="audio-speed">Speed <select data-audio-speed aria-label="Playback speed" value={$model.speed} onchange={event => player.speed(Number(event.currentTarget.value))}>
      {#each SPEEDS as speed}<option value={speed}>{speed}×</option>{/each}
    </select></label>
    <div class="audio-volume">
      <button type="button" data-audio-mute aria-label={$model.muted ? 'Unmute' : 'Mute'} title={$model.muted ? 'Unmute' : 'Mute'} aria-pressed={$model.muted} onclick={player.mute}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M11 5 6 9H3v6h3l5 4z" />
          {#if $model.muted}<path class="audio-volume-wave" d="m16 9 5 6m0-6-5 6" />{:else}<path class="audio-volume-wave" d="M15 8a6 6 0 0 1 0 8m3-11a10 10 0 0 1 0 14" />{/if}
        </svg>
      </button>
      {#if $model.volumeAdjustable}<input data-audio-volume type="range" min="0" max="1" step="0.05" value={$model.volume} aria-label="Volume" aria-valuetext={`${Math.round($model.volume*100)}%`} oninput={event => player.volume(Number(event.currentTarget.value))} />
      {:else}<span title="Use the volume buttons on your device">Device volume</span>{/if}
    </div>
  </div>
</div>
