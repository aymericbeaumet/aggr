/**
 * Dates as the reader shows them.
 *
 * `formatDate` is `{{ value | date }}` from the templates (`src/site/render.rs`): the calendar
 * day of an RFC 3339 timestamp in the timestamp's own offset. The archive stores every date in
 * UTC, so that is the first ten characters. Anything that is not an RFC 3339 timestamp passes
 * through unchanged, like the Rust filter. It is what a server render and a page without a
 * running clock show; the other styles follow the `date-format` preference in the browser.
 */
const RFC3339 = /^(\d{4}-\d{2}-\d{2})[Tt ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:[Zz]|[+-]\d{2}:\d{2})$/;

export function formatDate(value: string): string {
  const match = RFC3339.exec(value);
  return match ? match[1] : value;
}

/** The values of the `date-format` preference. */
export type DateStyle = 'relative' | 'iso' | 'local' | 'local-time';

const formatters = new Map<string, Intl.DateTimeFormat>();

function formatter(name: string, options: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  let value = formatters.get(name);
  if (!value) formatters.set(name, (value = new Intl.DateTimeFormat(undefined, options)));
  return value;
}

/** `3h ago`: the age of `timestamp` at `now`, in the coarsest unit that still says something. */
export function relativeText(timestamp: number, now: number): string {
  const seconds = Math.max(0, Math.round((now - timestamp) / 1000));
  if (seconds < 60) return 'just now';
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 45) return `${days}d ago`;
  const months = Math.floor(days / 30);
  if (months < 18) return `${months}mo ago`;
  return `${Math.floor(days / 365)}y ago`;
}

/**
 * The text of a `<time>` in one of the preference's styles, or null when `timestamp` is not a
 * date. An unknown style is relative, the preference's default.
 */
export function formatTimestamp(timestamp: number, style: string, now: number): string | null {
  if (!Number.isFinite(timestamp)) return null;
  switch (style) {
    case 'iso':
      return new Date(timestamp).toISOString().slice(0, 10);
    case 'local':
      return formatter('local', { dateStyle: 'medium' }).format(timestamp);
    case 'local-time':
      return formatter('local-time', { dateStyle: 'medium', timeStyle: 'short' }).format(timestamp);
    default:
      return relativeText(timestamp, now);
  }
}

/** The tooltip of a published date as the template writes it: the exact timestamps. */
export function publishedTooltip(exact: string, updated?: string | null): string {
  return `Published: ${exact}${updated ? `\nUpdated: ${updated}` : ''}`;
}

/**
 * The same tooltip in the reader's locale, built only when someone looks at it: the exact,
 * localized timestamp costs a formatter. An update at the published instant says nothing.
 */
export function localizedTooltip(timestamp: number, updated: number | null): string {
  const exact = formatter('tooltip', {
    weekday: 'long',
    year: 'numeric',
    month: 'long',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
  let text = `Published: ${exact.format(timestamp)}`;
  if (updated !== null && Number.isFinite(updated) && updated !== timestamp) {
    text += `\nUpdated: ${exact.format(updated)}`;
  }
  return text;
}

/** The bands the stylesheet paints rows in as their articles age. */
export type AgeBand = 'fresh' | 'h1' | 'h3' | 'h24';

export function ageBand(published: number, now: number): AgeBand {
  const age = Math.max(0, now - published);
  const hour = 3_600_000;
  return age < hour ? 'fresh' : age < 3 * hour ? 'h1' : age < 24 * hour ? 'h3' : 'h24';
}
