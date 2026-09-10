import type { ItemMetadata, ResultData } from './types';
import { relativeDate } from '../dates';

export interface Display extends ItemMetadata {
  excerpt?: string;
  preview?: { url: string; width: number; height: number; alt?: string; color?: string; placeholder?: { hash: string; data_url: string } };
}

export function placeholderBackground(value: string | undefined): string | undefined {
  return value && value.length <= 8192 && /^data:image\/png;base64,[A-Za-z0-9+/]+={0,2}$/.test(value)
    ? `url("${value}")` : undefined;
}
function decodeDisplay(result: ResultData): Display {
  try {
    const hex = result.meta.aggr_display || '';
    if (!/^(?:[0-9a-f]{2})+$/i.test(hex)) return {};
    const bytes = Uint8Array.from(hex.match(/../g)!, part => Number.parseInt(part, 16));
    const value = JSON.parse(new TextDecoder().decode(bytes));
    return value && typeof value === 'object' && !Array.isArray(value) ? value : {};
  } catch { return {}; }
}

const displays = new WeakMap<ResultData, Display>();
export function displayData(result: ResultData): Display {
  let display = displays.get(result);
  if (!display) { display = decodeDisplay(result); displays.set(result, display); }
  return display;
}

export function safeURL(value: string | undefined, base: string): string {
  try {
    const url = new URL(value || '', base);
    return ['http:', 'https:'].includes(url.protocol) ? url.href : base;
  } catch { return base; }
}

const dateFormatters = new Map<string, Intl.DateTimeFormat>();
function dateFormatter(key: string, options: Intl.DateTimeFormatOptions) {
  let formatter = dateFormatters.get(key);
  if (!formatter) { formatter = new Intl.DateTimeFormat(undefined, options); dateFormatters.set(key, formatter); }
  return formatter;
}

export function dateLabel(date: string, format: unknown, now = Date.now()): string {
  const timestamp = Date.parse(date);
  if (!Number.isFinite(timestamp)) return date.slice(0, 10);
  if (format === 'iso') return new Date(timestamp).toISOString().slice(0, 10);
  if (format === 'local') return dateFormatter('local', { dateStyle: 'medium' }).format(timestamp);
  if (format === 'local-time') return dateFormatter('local-time', { dateStyle: 'medium', timeStyle: 'short' }).format(timestamp);
  return relativeDate(timestamp, now) || date.slice(0, 10);
}

export function dateTooltip(published: string, updated: string | undefined, localized = false): string {
  const exact = (value: string) => {
    const timestamp = Date.parse(value);
    return localized && Number.isFinite(timestamp)
      ? dateFormatter('tooltip', { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit' }).format(timestamp)
      : value;
  };
  const distinct = updated && Number.isFinite(Date.parse(updated)) && Date.parse(updated) !== Date.parse(published);
  return `Published: ${exact(published)}${distinct ? `\nUpdated: ${exact(updated)}` : ''}`;
}

interface ExcerptPart { readonly text: string; readonly highlight: boolean }
function excerptParts(html: string | undefined, fallback = ''): readonly ExcerptPart[] {
  if (!html) return [{ text: fallback, highlight: false }];
  const parsed = new DOMParser().parseFromString(html, 'text/html');
  const walker = parsed.createTreeWalker(parsed.body, NodeFilter.SHOW_TEXT);
  const parts: { text: string; highlight: boolean }[] = [];
  while (walker.nextNode()) {
    const node = walker.currentNode;
    if (node.parentElement?.closest('script,style')) continue;
    parts.push({ text: node.textContent || '', highlight: !!node.parentElement?.closest('mark') });
  }
  return parts;
}

const excerpts = new WeakMap<ResultData, readonly ExcerptPart[]>();
export function resultExcerptParts(result: ResultData): readonly ExcerptPart[] {
  let parts = excerpts.get(result);
  if (!parts) { parts = excerptParts(result.excerpt, displayData(result).excerpt); excerpts.set(result, parts); }
  return parts;
}
