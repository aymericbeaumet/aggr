import { createPlayback, type PlaybackState } from './audio';
import { claimVisual, connectVisual, type Episode } from './now-playing';
import { duration, timingHost } from './timing';

/**
 * A native `<video>` joins the same now-playing session as a podcast. The picture is seated once
 * and scales between the article and the bar. A video that never played is released with the page.
 */
export function watchNativeVideo(video: HTMLVideoElement, signal: AbortSignal): void {
  const frame = video.closest('.native-video');
  if (!(frame instanceof HTMLElement) || signal.aborted || !video.src) return;
  const episode: Episode = {
    src: video.src,
    title: video.getAttribute('aria-label')?.trim() || 'Video',
    show: '',
    artwork: video.poster || '',
    page: typeof location === 'undefined' ? '' : location.href.split('#')[0],
  };
  const report = (state: PlaybackState) => {
    if (!signal.aborted) timingHost(frame)(state);
  };
  if (claimVisual(frame, episode, report, signal)) return;

  frame.dataset.playingSrc = episode.src;
  const box: { push: (state: PlaybackState) => void } = { push() {} };
  const playback = createPlayback(
    video,
    (state) => box.push(state),
    { duration: duration(Number(video.dataset.durationSeconds)) },
  );
  box.push = connectVisual(video, episode, playback, report, signal, { controls: true }).push;
}
