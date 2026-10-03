import { describe, expect, it } from 'vitest';
import {
  ageBand,
  formatDate,
  formatTimestamp,
  localizedTooltip,
  publishedTooltip,
  relativeText,
} from './format';

describe('formatDate', () => {
  it('keeps the calendar day of an RFC 3339 timestamp', () => {
    expect(formatDate('2026-09-30T07:00:00+00:00')).toBe('2026-09-30');
    expect(formatDate('2026-09-28T12:00:00Z')).toBe('2026-09-28');
    expect(formatDate('2026-01-02T23:59:59.250+02:00')).toBe('2026-01-02');
  });

  it('passes other values through', () => {
    expect(formatDate('yesterday')).toBe('yesterday');
    expect(formatDate('2026-09-30')).toBe('2026-09-30');
    expect(formatDate('')).toBe('');
  });
});

describe('relativeText', () => {
  const now = Date.parse('2026-09-30T12:00:00Z');
  const ago = (ms: number) => relativeText(now - ms, now);
  const minute = 60_000;
  const hour = 60 * minute;
  const day = 24 * hour;

  it('steps through the units the vanilla reader used', () => {
    expect(ago(0)).toBe('just now');
    expect(ago(59_000)).toBe('just now');
    expect(ago(minute)).toBe('1m ago');
    expect(ago(59 * minute)).toBe('59m ago');
    expect(ago(hour)).toBe('1h ago');
    expect(ago(23 * hour)).toBe('23h ago');
    expect(ago(day)).toBe('1d ago');
    expect(ago(44 * day)).toBe('44d ago');
    expect(ago(45 * day)).toBe('1mo ago');
    expect(ago(17 * 30 * day)).toBe('17mo ago');
    expect(ago(18 * 30 * day)).toBe('1y ago');
    expect(ago(3 * 365 * day)).toBe('3y ago');
  });

  it('treats the future as now', () => {
    expect(relativeText(now + day, now)).toBe('just now');
  });
});

describe('formatTimestamp', () => {
  const now = Date.parse('2026-09-30T12:00:00Z');
  const timestamp = Date.parse('2026-09-29T08:30:00Z');

  it('follows the preference styles', () => {
    expect(formatTimestamp(timestamp, 'relative', now)).toBe('1d ago');
    expect(formatTimestamp(timestamp, 'iso', now)).toBe('2026-09-29');
    expect(formatTimestamp(timestamp, 'local', now)).toBe(
      new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' }).format(timestamp),
    );
    expect(formatTimestamp(timestamp, 'local-time', now)).toBe(
      new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(timestamp),
    );
  });

  it('is relative for an unknown style and null for a non-date', () => {
    expect(formatTimestamp(timestamp, 'bogus', now)).toBe('1d ago');
    expect(formatTimestamp(Number.NaN, 'iso', now)).toBeNull();
  });
});

describe('tooltips', () => {
  it('writes the exact timestamps like the template', () => {
    expect(publishedTooltip('2026-09-30T07:00:00+00:00')).toBe('Published: 2026-09-30T07:00:00+00:00');
    expect(publishedTooltip('a', 'b')).toBe('Published: a\nUpdated: b');
    expect(publishedTooltip('a', null)).toBe('Published: a');
  });

  it('localizes on demand and drops an update at the published instant', () => {
    const published = Date.parse('2026-09-30T07:00:00Z');
    const updated = Date.parse('2026-09-30T11:00:00Z');
    const text = localizedTooltip(published, updated);
    expect(text.startsWith('Published: ')).toBe(true);
    expect(text).toContain('\nUpdated: ');
    expect(localizedTooltip(published, published)).not.toContain('Updated');
    expect(localizedTooltip(published, null)).not.toContain('Updated');
  });
});

describe('ageBand', () => {
  const now = Date.parse('2026-09-30T12:00:00Z');
  const hour = 3_600_000;

  it('matches the build-time bands', () => {
    expect(ageBand(now, now)).toBe('fresh');
    expect(ageBand(now - hour + 1, now)).toBe('fresh');
    expect(ageBand(now - hour, now)).toBe('h1');
    expect(ageBand(now - 3 * hour, now)).toBe('h3');
    expect(ageBand(now - 24 * hour, now)).toBe('h24');
    expect(ageBand(now + hour, now)).toBe('fresh');
  });
});
