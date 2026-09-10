interface SharedDates {
  text(timestamp: number, format: string): string | null;
}

interface TimeState {
  exact: string;
  timestamp: number;
  updated: number;
  local: string;
  text?: string;
  localized?: boolean;
}

export interface DateOptions {
  format(): string;
  afterFormat(root: ParentNode): void;
}

export function relativeDate(timestamp: number, now = Date.now()): string | null {
  if (Number.isNaN(timestamp)) return null;
  const seconds = Math.max(0, Math.round((now - timestamp) / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.ceil(hours / 24);
  if (days < 45) return `${days}d ago`;
  const months = Math.floor(days / 30);
  if (months < 18) return `${months}mo ago`;
  return `${Math.floor(days / 365)}y ago`;
}

export function distinctUpdatedTimestamp(published: number, updated: string): number {
  const timestamp = Date.parse(updated);
  return timestamp === published ? NaN : timestamp;
}

export function createDates(options: DateOptions) {
  const formatters = new Map<string, Intl.DateTimeFormat>();
  const states = new WeakMap<HTMLTimeElement, TimeState>();
  const listeners = new AbortController();

  function formatter(name: string, settings: Intl.DateTimeFormatOptions) {
    let value = formatters.get(name);
    if (!value) {
      value = new Intl.DateTimeFormat(undefined, settings);
      formatters.set(name, value);
    }
    return value;
  }

  function text(timestamp: number, format: string): string | null {
    const shared = (window as Window & { AGGRDates?: SharedDates }).AGGRDates;
    if (shared) return shared.text(timestamp, format);
    if (Number.isNaN(timestamp)) return null;
    if (format === "iso") return new Date(timestamp).toISOString().slice(0, 10);
    if (format === "local") return formatter("local", { dateStyle: "medium" }).format(timestamp);
    if (format === "local-time") {
      return formatter("local-time", { dateStyle: "medium", timeStyle: "short" }).format(timestamp);
    }
    return relativeDate(timestamp);
  }

  function formatTimes(root: ParentNode = document) {
    const format = options.format();
    for (const time of root.querySelectorAll<HTMLTimeElement>("time[datetime]")) {
      if (time.closest("[data-search-results]")) continue;
      const exact = time.getAttribute("datetime") || "";
      let state = states.get(time);
      if (!state || state.exact !== exact) {
        const timestamp = Date.parse(exact);
        if (Number.isNaN(timestamp)) continue;
        const label = time.closest<HTMLElement>("[data-date-tooltip]");
        const updated = label?.dataset.dateUpdated || "";
        const updatedTimestamp = distinctUpdatedTimestamp(timestamp, updated);
        const tooltip = label
          ? `Published: ${exact}${!Number.isNaN(updatedTimestamp) ? `\nUpdated: ${updated}` : ""}`
          : exact;
        state = { exact, timestamp, updated: updatedTimestamp, local: tooltip };
        states.set(time, state);
        if (label) {
          label.title = tooltip;
          time.removeAttribute("title");
        } else time.title = tooltip;
      }
      const label = text(state.timestamp, format);
      if (label && state.text !== label) {
        time.setAttribute("aria-label", `${label}; ${state.local}`);
        if (time.textContent !== label) time.textContent = label;
        state.text = label;
      }
    }
    options.afterFormat(root);
  }

  function localizeTooltip(time: HTMLTimeElement) {
    const state = states.get(time);
    if (!state || state.localized) return;
    const localized = formatter("tooltip", {
      weekday: "long", year: "numeric", month: "long", day: "numeric",
      hour: "2-digit", minute: "2-digit", second: "2-digit",
    });
    const label = time.closest<HTMLElement>("[data-date-tooltip]");
    state.local = `${label ? "Published: " : ""}${localized.format(state.timestamp)}`;
    if (!Number.isNaN(state.updated)) state.local += `\nUpdated: ${localized.format(state.updated)}`;
    state.localized = true;
    (label || time).title = state.local;
    time.setAttribute("aria-label", `${state.text}; ${state.local}`);
  }

  for (const name of ["pointerover", "focusin"]) {
    document.addEventListener(name, (event) => {
      const label = event.target instanceof Element
        ? event.target.closest("[data-date-tooltip], time[datetime]") : null;
      if (!label || label.closest("[data-search-results]")) return;
      const time = label instanceof HTMLTimeElement ? label : label.querySelector<HTMLTimeElement>("time[datetime]");
      if (time) localizeTooltip(time);
    }, { passive: true, signal: listeners.signal });
  }

  return { formatTimes, dispose: () => listeners.abort() };
}
