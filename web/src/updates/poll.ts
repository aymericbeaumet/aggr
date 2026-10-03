import type { Build } from '../state/versions.svelte';

/** `updates.json` as the build writes it; anything else is ignored rather than acted on. */
export function parseBuild(value: unknown): Build | null {
  if (typeof value !== 'object' || value === null) return null;
  const { app_version, content_version, entries } = value as Record<string, unknown>;
  if (typeof app_version !== 'string' || typeof content_version !== 'string') return null;
  const build: Build = { app_version, content_version };
  if (Array.isArray(entries)) build.entries = entries.filter((entry): entry is string => typeof entry === 'string');
  return build;
}

export type PollerOptions = {
  /** Fetch and decode `updates.json`; a rejection or a non-build value is a check that found nothing. */
  load: (signal: AbortSignal) => Promise<unknown>;
  onBuild: (build: Build) => void;
  /** Whether a check is worth making now: online, visible, and a page with a version to compare. */
  active: () => boolean;
  /** Milliseconds between checks while active. The browser suite shortens exactly this delay. */
  interval?: number;
  timeout?: number;
};

/** The deployment check is the one polling interval the browser suite shortens: keep it exactly this. */
export const POLL_INTERVAL = 15_000;

/**
 * Checks `updates.json` on an interval and on demand, with one request in flight at a time:
 * a check asked for while one is running shares it, or queues one more when it insists (the
 * dev server's rebuild event). A completion after `stop` is stale and changes nothing.
 */
export class Poller {
  private inflight: Promise<void> | null = null;
  private queued = false;
  private generation = 0;
  private timer: ReturnType<typeof setInterval> | undefined;
  private readonly interval: number;
  private readonly timeout: number;

  constructor(private readonly options: PollerOptions) {
    this.interval = options.interval ?? POLL_INTERVAL;
    this.timeout = options.timeout ?? 10_000;
  }

  /** Check now, or share the check running. `queue` asks for one more once that one is done. */
  check(queue = false): Promise<void> {
    if (!this.options.active()) return Promise.resolve();
    if (this.inflight) {
      this.queued ||= queue;
      return this.inflight;
    }
    this.queued = false;
    const generation = this.generation;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeout);
    const run = this.options
      .load(controller.signal)
      .then((value) => {
        const build = parseBuild(value);
        if (build && generation === this.generation) this.options.onBuild(build);
      })
      .catch(() => {})
      .finally(() => {
        clearTimeout(timer);
        if (this.inflight === run) this.inflight = null;
        if (generation === this.generation && this.queued) void this.check();
      });
    this.inflight = run;
    return run;
  }

  start(): void {
    if (this.timer !== undefined) return;
    this.timer = setInterval(() => void this.check(), this.interval);
  }

  /** End the interval and disown the check in flight. */
  stop(): void {
    clearInterval(this.timer);
    this.timer = undefined;
    this.generation += 1;
    this.queued = false;
  }
}
