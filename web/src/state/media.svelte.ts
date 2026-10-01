import type { Consumption } from '../generated/Consumption';

/**
 * The lengths native players have reported, by the article's original link. The build writes
 * "Watch" or "Listen" when the archive has no reliable duration; once the file itself says how
 * long it is, the metadata shows "N min watch" in the same words the build would have used.
 */
class MediaDurations {
  known = $state.raw<Record<string, number>>({});

  /** A player has loaded `original` and knows its length in seconds. */
  report(original: string, seconds: number): void {
    if (!original || !Number.isFinite(seconds) || seconds <= 0 || this.known[original] === seconds) return;
    this.known = { ...this.known, [original]: seconds };
  }

  /** `consumption` with the reported length of `original`; reading time is never a recording's. */
  correct(consumption: Consumption | undefined, original: string): Consumption | undefined {
    const seconds = this.known[original];
    if (!consumption || consumption.action === 'read' || seconds === undefined) return consumption;
    const whole = Math.ceil(seconds);
    return { ...consumption, seconds: whole, minutes: Math.ceil(whole / 60) };
  }
}

export const mediaDurations = new MediaDurations();
