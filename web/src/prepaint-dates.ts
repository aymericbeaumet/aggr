import { relativeDate } from './dates';

export function createDateText() {
  const formatters = new Map<string, Intl.DateTimeFormat>();
  function formatter(name: string, options: Intl.DateTimeFormatOptions) {
    if (!formatters.has(name)) formatters.set(name, new Intl.DateTimeFormat(undefined, options));
    return formatters.get(name)!;
  }
  return function text(timestamp: number, format: string) {
    if (!Number.isFinite(timestamp) || Number.isNaN(new Date(timestamp).getTime())) return null;
    if (format === 'iso') return new Date(timestamp).toISOString().slice(0, 10);
    if (format === 'local') return formatter('local', { dateStyle: 'medium' }).format(timestamp);
    if (format === 'local-time') return formatter('local-time', { dateStyle: 'medium', timeStyle: 'short' }).format(timestamp);
    return relativeDate(timestamp);
  };
}

export function formatBeforePaint(document: Document, text: (timestamp: number) => string | null) {
  function format(time: HTMLTimeElement) {
    if (!time.textContent) return;
    const value = text(Date.parse(time.getAttribute('datetime') || ''));
    if (value && time.textContent !== value) time.textContent = value;
  }
  function collect(node: Node, times: Set<HTMLTimeElement>, descendants: boolean) {
    const element = node.nodeType === 1 ? node as Element : node.parentElement;
    if (!element) return;
    const time = element.closest<HTMLTimeElement>('time[datetime]');
    if (time) times.add(time);
    if (descendants && node.nodeType === 1) {
      element.querySelectorAll<HTMLTimeElement>('time[datetime]').forEach(time => times.add(time));
    }
  }
  function changes(records: MutationRecord[]) {
    const times = new Set<HTMLTimeElement>();
    for (const record of records) {
      collect(record.target, times, false);
      record.addedNodes.forEach(node => collect(node, times, true));
    }
    times.forEach(format);
  }
  function all() { document.querySelectorAll<HTMLTimeElement>('time[datetime]').forEach(format); }
  if (document.readyState === 'loading') {
    const observer = new MutationObserver(changes);
    observer.observe(document.documentElement, { childList: true, subtree: true, characterData: true });
    document.addEventListener('DOMContentLoaded', () => {
      changes(observer.takeRecords());
      observer.disconnect();
      all();
    }, { once: true });
  }
  all();
}
