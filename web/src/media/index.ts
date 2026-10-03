import { mediaDurations } from '../state/media.svelte';
import { page } from '../state/page.svelte';
import { enhanceAudio } from './audio';
import { enhanceVideoFacade } from './facades';
import { duration, followNativeVideo } from './timing';

/** The archived length gives way to the file's own once a native player has loaded it. */
function reportDuration(media: HTMLMediaElement, original: string, signal: AbortSignal): void {
  if (!original) return;
  const report = () => {
    const seconds = duration(media.duration);
    if (seconds !== undefined) mediaDurations.report(original, seconds);
  };
  for (const name of ['loadedmetadata', 'durationchange']) media.addEventListener(name, report, { signal });
  report();
}

/**
 * Players and their timing readouts, loaded only for an article whose content region carries
 * media. Every control driven here is already in the HTML, hidden until enhancement succeeds,
 * so a failure leaves working native controls rather than nothing. `signal` ends the players:
 * playback stops and their resources are released when the page is left.
 */
export function mount(region: ParentNode, signal: AbortSignal): void {
  if (signal.aborted) return;
  const model = page.model;
  const original = model?.page.view === 'article' ? model.page.data.header.metadata.original : '';
  for (const card of region.querySelectorAll('[data-audio-component]')) {
    if (!(card instanceof HTMLElement)) continue;
    enhanceAudio(card, signal);
    const audio = card.querySelector('audio');
    if (audio instanceof HTMLAudioElement) reportDuration(audio, original, signal);
  }
  for (const video of region.querySelectorAll('.native-video video')) {
    if (!(video instanceof HTMLVideoElement)) continue;
    followNativeVideo(video, signal);
    reportDuration(video, original, signal);
  }
  for (const preview of region.querySelectorAll('[data-video-embed]')) {
    if (preview instanceof HTMLAnchorElement) enhanceVideoFacade(preview, signal);
  }
}
