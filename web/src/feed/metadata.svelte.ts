import { formatTimestamp, localizedTooltip, publishedTooltip } from '../dates/format';
import { ageBand } from '../dates/format';
import { clock } from '../state/clock.svelte';
import { mediaDurations } from '../state/media.svelte';
import { page } from '../state/page.svelte';
import { preferences } from '../state/preferences.svelte';
import { search } from '../state/search.svelte';

/** Only changing values are enhanced; the metadata structure belongs to the Rust partial. */
export function installMetadata(root: HTMLElement): () => void {
  const controller = new AbortController();
  const tooltip = (event: Event) => {
    const target = event.target instanceof Element ? event.target.closest<HTMLElement>('[data-date-tooltip]') : null;
    const time = target?.querySelector<HTMLTimeElement>('time[datetime]');
    if (!target || !time) return;
    const timestamp = Date.parse(time.dateTime);
    const updated = target.dataset.dateUpdated ? Date.parse(target.dataset.dateUpdated) : null;
    if (Number.isFinite(timestamp)) target.title = localizedTooltip(timestamp, updated);
  };
  root.addEventListener('pointerover', tooltip, { signal: controller.signal });
  root.addEventListener('focusin', tooltip, { signal: controller.signal });
  const stop = $effect.root(() => {
    $effect(() => {
      void page.model;
      void search.results;
      const now = clock.now;
      const style = preferences.dateFormat;
      if (now === null || style === undefined) return;
      for (const time of root.querySelectorAll<HTMLTimeElement>('time.dt-published[datetime]')) {
        const timestamp = Date.parse(time.dateTime);
        if (!Number.isFinite(timestamp)) continue;
        const text = formatTimestamp(timestamp, style, now);
        time.textContent = text;
        const wrapper = time.closest<HTMLElement>('[data-date-tooltip]');
        const title = publishedTooltip(time.dateTime, wrapper?.dataset.dateUpdated);
        time.setAttribute('aria-label', `${text}; ${title}`);
        const row = time.closest<HTMLElement>('.row');
        if (row) {
          const band = ageBand(timestamp, now);
          for (const name of [...row.classList]) if (name.startsWith('age-')) row.classList.remove(name);
          row.classList.add(`age-${band}`);
          row.dataset.age = band;
        }
      }
    });
    $effect(() => {
      void page.model;
      void search.results;
      const durations = mediaDurations.known;
      for (const stats of root.querySelectorAll<HTMLElement>('[data-consumption]')) {
        if (stats.dataset.consumption === 'read') continue;
        const original = stats.closest('.meta')?.querySelector<HTMLAnchorElement>('.u-bookmark-of')?.href;
        const seconds = original ? durations[original] : undefined;
        if (seconds === undefined) continue;
        const whole = Math.ceil(seconds);
        stats.dataset.durationSeconds = String(whole);
        let time = stats.querySelector('time');
        if (!time) { time = document.createElement('time'); stats.replaceChildren(time); }
        time.dateTime = `PT${whole}S`;
        time.textContent = `${Math.ceil(whole / 60)} min ${stats.dataset.consumption}`;
      }
    });
  });
  return () => { controller.abort(); stop(); };
}
