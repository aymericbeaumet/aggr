import { mediaDurations } from '../state/media.svelte';
import { page } from '../state/page.svelte';
import { enhanceAudio } from './audio';
import { enhanceVideoFacade } from './facades';
import { duration } from './timing';
import { watchNativeVideo } from './visual';

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
 * so a failure leaves working native controls rather than nothing. `signal` releases media that
 * has not started. Podcasts, native video and provider players that have started keep playing
 * from that same element; leaving the site or dismissing the player stops it.
 */
export function mount(region: ParentNode, signal: AbortSignal): void {
  if (signal.aborted) return;
  const model = page.model;
  const original = model?.page.view === 'article' ? model.page.data.header.metadata.original : '';
  for (const card of region.querySelectorAll('[data-audio-component]')) {
    if (!(card instanceof HTMLElement)) continue;
    // Captured first: playback may later move the element out of the card.
    const audio = card.querySelector('audio');
    enhanceAudio(card, signal);
    if (audio instanceof HTMLAudioElement) reportDuration(audio, original, signal);
  }
  for (const video of region.querySelectorAll('.native-video video')) {
    if (!(video instanceof HTMLVideoElement)) continue;
    watchNativeVideo(video, signal);
    reportDuration(video, original, signal);
  }
  for (const preview of region.querySelectorAll('[data-video-embed]')) {
    if (preview instanceof HTMLAnchorElement) enhanceVideoFacade(preview, signal);
  }
}
