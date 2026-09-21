// @ts-check
/**
 * The reader's enhancements. Everything here is optional: the generated HTML is complete and
 * navigable on its own, and this file only adds behaviour the platform cannot express in markup.
 *
 * No build step, no dependencies. Editors type-check it through jsconfig.json; `types/aggr.d.ts`
 * declares the contracts this file shares with the templates.
 */

/** @typedef {import("../../../types/aggr").AppContext} AppContext */

const AGGR = /** @type {AppContext} */ (window.AGGR || {});
const PREFS = window.AGGRPreferences;
const BASE = new URL(AGGR.base || "./", document.baseURI).href;
const KIND = AGGR.kind || document.body.dataset.kind || "";

/**
 * @template {Element} [T=HTMLElement]
 * @param {string} selector
 * @param {ParentNode} [root]
 * @returns {T | null}
 */
const $ = (selector, root = document) => /** @type {T | null} */ (root.querySelector(selector));

/**
 * @template {Element} [T=HTMLElement]
 * @param {string} selector
 * @param {ParentNode} [root]
 * @returns {T[]}
 */
const $$ = (selector, root = document) => Array.from(root.querySelectorAll(selector));

/** Never let one broken enhancement take the rest of the page down with it. */
function safely(label, run) {
  try {
    return run();
  } catch (error) {
    console.error("aggr: " + label, error);
    return undefined;
  }
}

const storage = {
  /** @param {Storage} store @param {string} key */
  read(store, key) {
    try {
      return store.getItem(key);
    } catch {
      return null; // private mode
    }
  },
  /** @param {Storage} store @param {string} key @param {string} value */
  write(store, key, value) {
    try {
      store.setItem(store === localStorage ? key : key, value);
      return true;
    } catch {
      return false;
    }
  },
};

/** Per-site key, so several readers on one origin never share session state. */
const scopeKey = (name) => "aggr:" + name + ":" + encodeURIComponent(new URL(BASE).pathname);

/* ------------------------------------------------------------------ shift-hover */

/** While Shift is held, links reveal where they lead. Styled entirely by the stylesheet. */
function installShiftHover() {
  const held = (on) => document.documentElement.classList.toggle("is-shift-held", on);
  const options = { capture: true, passive: true };
  document.addEventListener("keydown", (event) => held(event.shiftKey), options);
  document.addEventListener("keyup", (event) => held(event.shiftKey), options);
  document.addEventListener("pointerover", (event) => held(event.shiftKey), options);
  document.addEventListener("visibilitychange", () => document.hidden && held(false), options);
  window.addEventListener("blur", () => held(false), options);
  window.addEventListener("pagehide", () => held(false), options);
}

/* ------------------------------------------------------------------ dates */

/**
 * Dates were already written during parsing by the inline bootstrap in `_dates.html`. This keeps
 * them current while the tab stays open and upgrades the tooltip to a localized one on demand.
 */
const dates = (() => {
  const formatters = new Map();
  /** @type {WeakMap<HTMLTimeElement, {exact: string, timestamp: number, updated: number, tooltip: string, text?: string, localized?: boolean}>} */
  const states = new WeakMap();

  const formatter = (name, settings) => {
    let value = formatters.get(name);
    if (!value) formatters.set(name, (value = new Intl.DateTimeFormat(undefined, settings)));
    return value;
  };

  const format = () => String(PREFS?.values["date-format"] || "relative");

  function text(timestamp, style) {
    if (window.AGGRDates) return window.AGGRDates.text(timestamp, style);
    if (Number.isNaN(timestamp)) return null;
    if (style === "iso") return new Date(timestamp).toISOString().slice(0, 10);
    if (style === "local") return formatter("local", { dateStyle: "medium" }).format(timestamp);
    if (style === "local-time")
      return formatter("local-time", { dateStyle: "medium", timeStyle: "short" }).format(timestamp);
    const seconds = Math.max(0, Math.round((Date.now() - timestamp) / 1000));
    if (seconds < 60) return "just now";
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return minutes + "m ago";
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return hours + "h ago";
    const days = Math.ceil(hours / 24);
    if (days < 45) return days + "d ago";
    const months = Math.floor(days / 30);
    if (months < 18) return months + "mo ago";
    return Math.floor(days / 365) + "y ago";
  }

  /** @param {ParentNode} [root] */
  function render(root = document) {
    const style = format();
    for (const time of $$("time[datetime]", root)) {
      const element = /** @type {HTMLTimeElement} */ (time);
      const exact = element.getAttribute("datetime") || "";
      let state = states.get(element);
      if (!state || state.exact !== exact) {
        const timestamp = Date.parse(exact);
        if (Number.isNaN(timestamp)) continue;
        const label = element.closest("[data-date-tooltip]");
        const updated = label instanceof HTMLElement ? label.dataset.dateUpdated || "" : "";
        const updatedAt = Date.parse(updated) === timestamp ? NaN : Date.parse(updated);
        const tooltip = label
          ? "Published: " + exact + (Number.isNaN(updatedAt) ? "" : "\nUpdated: " + updated)
          : exact;
        state = { exact, timestamp, updated: updatedAt, tooltip };
        states.set(element, state);
        if (label instanceof HTMLElement) {
          label.title = tooltip;
          element.removeAttribute("title");
        } else element.title = tooltip;
      }
      const label = text(state.timestamp, style);
      if (label && state.text !== label) {
        element.setAttribute("aria-label", label + "; " + state.tooltip);
        if (element.textContent !== label) element.textContent = label;
        state.text = label;
      }
    }
    ageBands(root);
  }

  /** The exact, localized timestamp costs a formatter; build it only when someone looks. */
  function localize(time) {
    const state = states.get(time);
    if (!state || state.localized) return;
    const exact = formatter("tooltip", {
      weekday: "long", year: "numeric", month: "long", day: "numeric",
      hour: "2-digit", minute: "2-digit", second: "2-digit",
    });
    const label = time.closest("[data-date-tooltip]");
    state.tooltip = (label ? "Published: " : "") + exact.format(state.timestamp);
    if (!Number.isNaN(state.updated)) state.tooltip += "\nUpdated: " + exact.format(state.updated);
    state.localized = true;
    (label || time).title = state.tooltip;
    if (state.text) time.setAttribute("aria-label", state.text + "; " + state.tooltip);
  }

  for (const name of ["pointerover", "focusin"]) {
    document.addEventListener(
      name,
      (event) => {
        const target = event.target;
        if (!(target instanceof Element)) return;
        const label = target.closest("[data-date-tooltip], time[datetime]");
        const time = label instanceof HTMLTimeElement ? label : label?.querySelector("time[datetime]");
        if (time instanceof HTMLTimeElement) localize(time);
      },
      { passive: true },
    );
  }

  return { render, format };
})();

/** Recolour rows as their articles age, in the bands the stylesheet paints. */
function ageBands(root = document) {
  for (const row of $$(".rows:not(.search-results) .row", root)) {
    const time = $("time[datetime]", row);
    const published = Date.parse(time?.getAttribute("datetime") || "");
    if (Number.isNaN(published)) continue;
    const age = Math.max(0, Date.now() - published);
    const hour = 3600000;
    const band = age < hour ? "fresh" : age < 3 * hour ? "h1" : age < 24 * hour ? "h3" : "h24";
    row.classList.remove("age-fresh", "age-h1", "age-h3", "age-h24");
    row.classList.add("age-" + band);
    row.dataset.age = band;
  }
}

/* ------------------------------------------------------------------ list selection */

/** The keyboard cursor, remembered by article URL so a reordered list keeps the same row. */
const selection = (() => {
  const rows = () => $$(".rows:not([aria-busy='true']) .row:not([hidden])");
  const link = (row) => (row ? $("[data-row-open]", row) : null);

  function key() {
    const url = new URL(location.href);
    url.hash = "";
    return (
      "aggr:list-cursor:" +
      encodeURIComponent(new URL(BASE).pathname) +
      ":" +
      encodeURIComponent(url.href)
    );
  }
  const read = () => {
    try {
      return JSON.parse(storage.read(sessionStorage, key()) || "null") || {};
    } catch {
      return {};
    }
  };
  const write = (state) => storage.write(sessionStorage, key(), JSON.stringify(state));

  function select(row, focus, scroll) {
    const target = link(row);
    if (!target) return false;
    for (const entry of rows()) entry.classList.toggle("is-selected", entry === row);
    write({ ...read(), url: target.href, y: window.scrollY });
    if (focus) target.focus({ preventScroll: true });
    if (scroll) row.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "instant" });
    return true;
  }

  return {
    rows,
    link,
    select,
    edge(which) {
      const all = rows();
      return select((which === "first" ? all[0] : all.at(-1)) || null, true, true);
    },
    save() {
      const all = rows();
      if (!all.length) return;
      const selected = link(all.find((row) => row.classList.contains("is-selected")));
      write({ ...read(), ...(selected ? { url: selected.href } : {}), y: window.scrollY });
    },
    /** Reselect the remembered row, else the first one. Never steals focus on load. */
    restore() {
      const all = rows();
      if (!all.length) return;
      const wanted = read().url;
      const row = (wanted && all.find((entry) => link(entry)?.href === wanted)) || all[0];
      for (const entry of all) entry.classList.toggle("is-selected", entry === row);
    },
    move(direction) {
      const all = rows();
      if (!all.length) return false;
      const current = all.findIndex((row) => row.classList.contains("is-selected"));
      const next = current === -1 ? 0 : Math.min(all.length - 1, Math.max(0, current + direction));
      return select(all[next], true, true);
    },
    selected: () => rows().find((row) => row.classList.contains("is-selected")) || null,
  };
})();

/* ------------------------------------------------------------------ feed paging */

const FEED_PAGE = "feed-page";

const positiveInteger = (value, fallback) => {
  const parsed = Number.parseInt(String(value), 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
};

/** The address of `slice` of the static page at `target`, keeping the current page's parameters. */
function feedPageUrl(target, slice) {
  const current = new URL(location.href);
  const url = new URL(target, BASE);
  current.searchParams.forEach((value, name) => {
    if (name !== FEED_PAGE && !url.searchParams.has(name)) url.searchParams.set(name, value);
  });
  if (slice > 1) url.searchParams.set(FEED_PAGE, String(slice));
  else url.searchParams.delete(FEED_PAGE);
  return url.href;
}

/**
 * Show one preferred-size slice of the generated page. Every row stays in the document, so
 * crawlers and readers without JavaScript still see the complete static page.
 */
function applyFeedPaging() {
  const pager = $("[data-feed-pager]");
  const list = $(".rows:not(.search-results)");
  if (!pager || !list) return;
  const rows = $$(".row", list);
  const data = pager.dataset;

  const staticSize = positiveInteger(data.staticPageSize, rows.length || 1);
  const staticPage = positiveInteger(data.staticPage, 1);
  const staticPages = positiveInteger(data.staticPages, 1);
  const totalItems = Math.max(rows.length, positiveInteger(data.totalItems, rows.length));
  const pageSize = Math.min(positiveInteger(PREFS?.values["feed-page-size"], 50), staticSize);
  const slicesPerPage = Math.ceil(staticSize / pageSize);
  const slicesHere = Math.max(1, Math.ceil(rows.length / pageSize));
  const requested = new URL(location.href).searchParams.get(FEED_PAGE);
  const slice = Math.min(positiveInteger(requested, 1), slicesHere);
  const start = (slice - 1) * pageSize;
  const end = Math.min(start + pageSize, rows.length);

  rows.forEach((row, index) => {
    row.hidden = index < start || index >= end;
  });

  const lastCount = Math.max(0, totalItems - (staticPages - 1) * staticSize);
  const lastSlices = Math.max(1, Math.ceil(lastCount / pageSize));
  const totalPages = Math.max(1, (staticPages - 1) * slicesPerPage + lastSlices);
  const currentPage = (staticPage - 1) * slicesPerPage + slice;

  const links = {
    "[data-page-first]": { target: data.staticFirst, slice: 1, visible: currentPage > 1 },
    "[data-page-previous]": {
      target: slice > 1 ? location.href : data.staticPrevious,
      slice: slice > 1 ? slice - 1 : slicesPerPage,
      visible: currentPage > 1,
    },
    "[data-page-next]": {
      target: slice < slicesHere ? location.href : data.staticNext,
      slice: slice < slicesHere ? slice + 1 : 1,
      visible: currentPage < totalPages,
    },
    "[data-page-last]": { target: data.staticLast, slice: lastSlices, visible: currentPage < totalPages },
  };
  for (const [selector, link] of Object.entries(links)) {
    const node = /** @type {HTMLAnchorElement | null} */ ($(selector, pager));
    if (!node) continue;
    node.href = feedPageUrl(link.target || location.href, link.slice);
    node.hidden = !link.visible;
  }
  const status = $("[data-page-status]", pager);
  if (status) status.textContent = "page " + currentPage + " / " + totalPages;
  pager.hidden = totalPages <= 1;
}

/* ------------------------------------------------------------------ keyboard */

const editing = (target) =>
  target instanceof Element &&
  (target.closest("input, textarea, select, [contenteditable]:not([contenteditable='false'])") !== null ||
    ["textbox", "combobox", "searchbox"].includes(target.getAttribute("role") || ""));

const singleKeyShortcuts = () => PREFS?.values["single-key-shortcuts"] !== false;

/** The article on screen, as the external shortcuts see it. */
function currentArticle() {
  const article = $("article.item");
  if (article) {
    return {
      original: $("a.u-bookmark-of", article)?.getAttribute("href") || null,
      link: article.dataset.link || "",
      title: $(".p-name", article)?.textContent?.trim() || "",
      discussions: $$("[data-discussion]", article).map((node) => ({
        name: node.dataset.discussion || "",
        href: node.getAttribute("href") || "",
      })),
    };
  }
  const row = selection.selected();
  if (!row) return null;
  return {
    original: $("a.u-bookmark-of", row)?.getAttribute("href") || null,
    link: row.dataset.link || "",
    title: $(".p-name", row)?.textContent?.trim() || "",
    discussions: $$("[data-discussion]", row).map((node) => ({
      name: node.dataset.discussion || "",
      href: node.getAttribute("href") || "",
    })),
  };
}

/** `O` opens the original; a network's upper-case key opens its discussion, or its search. */
function externalTarget(key) {
  const article = currentArticle();
  if (!article) return null;
  if (key === "O") return article.original;
  const network = (AGGR.discussions || []).find((candidate) => candidate.shortcut === key);
  if (!network) return null;
  const rendered = article.discussions.find((link) => link.name === network.name);
  return rendered
    ? rendered.href
    : network.url
        .split("{url}").join(encodeURIComponent(article.link))
        .split("{title}").join(encodeURIComponent(article.title));
}

/** The `g` chord: `gg` to the top, letters to routes, digits to the numbered entries. */
function gotoTarget(key) {
  if (key === "g") return { top: true };
  const routes = { f: "", i: "", l: "browse/", p: "preferences/" };
  if (Object.prototype.hasOwnProperty.call(routes, key))
    return { url: new URL(routes[key], BASE).href };
  if (/^[1-9]$/.test(key)) {
    const entry = (AGGR.entries || [])[Number(key) - 1];
    if (entry) return { url: new URL(entry, BASE).href };
  }
  return null;
}

/** One line of prose, for the scroll keys to step by. */
function lineHeight() {
  const content = $(".body") || $("main");
  if (!content) return 24;
  const style = getComputedStyle(content);
  return parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.6 || 24;
}

function pageScroll(key, byLine) {
  const line = lineHeight();
  const header = ($(".itemhead") || $(".top"))?.getBoundingClientRect().height || 0;
  const visible = Math.max(line, window.innerHeight - header);
  const amount = positiveInteger(PREFS?.values["scroll-amount"], 10);
  const distance = byLine ? line : Math.min(line * amount, visible * 0.5);
  window.scrollBy({ top: key === "d" || key === "e" ? distance : -distance, behavior: "instant" });
}

let gotoArmed = false;
let gotoTimer;

function installKeyboard() {
  document.addEventListener("keydown", (event) => {
    if (event.defaultPrevented) return;
    const single = singleKeyShortcuts();
    const inEditor = editing(event.target);
    const dialog = $("dialog[open]");

    // Cmd/Ctrl+K reaches the search field from anywhere, including from inside an editor.
    if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === "k") {
      event.preventDefault();
      focusSearch();
      return;
    }

    // Scroll keys work with Ctrl even when single-key shortcuts are off.
    const key = event.key.length === 1 ? event.key.toLowerCase() : event.key;
    if (!event.altKey && !event.metaKey && !inEditor && !(event.ctrlKey && event.shiftKey)) {
      const byLine = event.ctrlKey && (key === "e" || key === "y");
      if ((byLine || ((key === "d" || key === "u") && !event.ctrlKey && single)) && !dialog) {
        event.preventDefault();
        pageScroll(key, byLine);
        return;
      }
    }

    if (inEditor || event.metaKey || event.ctrlKey || event.altKey || !single) return;

    if (gotoArmed) {
      gotoArmed = false;
      clearTimeout(gotoTimer);
      const target = gotoTarget(event.key);
      if (target) {
        event.preventDefault();
        if (target.top) {
          if (KIND === "item") window.scrollTo({ top: 0, behavior: "instant" });
          else selection.edge("first");
        } else location.assign(target.url);
      }
      return;
    }

    if (dialog) return;

    if (event.key === "?") {
      event.preventDefault();
      openShortcutHelp();
      return;
    }
    if (event.key === "g") {
      gotoArmed = true;
      gotoTimer = setTimeout(() => (gotoArmed = false), 1200);
      return;
    }
    if (event.key === "G") {
      event.preventDefault();
      if (KIND === "item") window.scrollTo({ top: document.body.scrollHeight, behavior: "instant" });
      else selection.edge("last");
      return;
    }
    if (key === "j" || key === "k") {
      const direction = key === "j" ? 1 : -1;
      if (KIND === "item") {
        const article = $("article.item");
        const url = direction === 1 ? article?.dataset.nextUrl : article?.dataset.previousUrl;
        if (url) {
          event.preventDefault();
          location.assign(new URL(url, BASE).href);
        }
        return;
      }
      if (selection.move(direction)) event.preventDefault();
      return;
    }
    if ((key === "o" || event.key === "Enter") && KIND !== "item") {
      const link = selection.link(selection.selected());
      if (link) {
        event.preventDefault();
        link.click();
      }
      return;
    }
    // Upper-case single keys open the original or a discussion.
    if (event.key.length === 1 && event.key === event.key.toUpperCase()) {
      const target = externalTarget(event.key);
      if (target) {
        event.preventDefault();
        window.open(target, "_blank", "noopener,noreferrer");
      }
    }
  });
}

/* ------------------------------------------------------------------ shortcut help */

function openShortcutHelp() {
  const dialog = /** @type {HTMLDialogElement | null} */ ($("#shortcut-help"));
  if (dialog && !dialog.open) dialog.showModal();
}

function installShortcutHelp() {
  const dialog = /** @type {HTMLDialogElement | null} */ ($("#shortcut-help"));
  if (!dialog) return;
  // Where invoker commands are unsupported, wire the button up by hand.
  if (!("command" in HTMLButtonElement.prototype)) {
    for (const button of $$("[commandfor='shortcut-help']"))
      button.addEventListener("click", (event) => {
        event.preventDefault();
        openShortcutHelp();
      });
  }
  // A click on the backdrop lands on the dialog itself.
  dialog.addEventListener("click", (event) => {
    if (event.target === dialog) dialog.close();
  });
}

/* ------------------------------------------------------------------ search field */

/** Focus the shared search field, coming home first when the current page has none. */
function focusSearch() {
  const field = /** @type {HTMLInputElement | null} */ ($("#q"));
  if (!field) {
    location.assign(new URL("?focus-search=1", BASE).href);
    return;
  }
  loadSearch();
  field.focus();
  field.select();
}

let searchModule;
/** The search engine is a separate file, fetched the first time someone reaches for it. */
function loadSearch() {
  if (searchModule || !AGGR.assets?.search || !$("[data-search-root]")) return searchModule;
  searchModule = import(new URL(AGGR.assets.search, document.baseURI).href)
    .then((module) => module.mount({ base: BASE, dates, selection, preferences: PREFS }))
    .catch((error) => console.error("aggr: search", error));
  return searchModule;
}

function installSearchIntent() {
  const toolbar = $(".feed-toolbar");
  if (!toolbar) return;
  for (const name of ["pointerover", "focusin", "touchstart"])
    toolbar.addEventListener(name, () => loadSearch(), { once: true, passive: true });
  const url = new URL(location.href);
  if (url.searchParams.has("q")) loadSearch();
  if (url.searchParams.has("focus-search")) {
    url.searchParams.delete("focus-search");
    history.replaceState(history.state, "", url.href);
    focusSearch();
  }
}

/* ------------------------------------------------------------------ media */

/** Players and their timing readouts only exist on article pages that carry media. */
function loadMedia() {
  if (!AGGR.assets?.media) return;
  if (!$(".video-player, [data-audio-component], .native-video, [data-media-timing]")) return;
  import(new URL(AGGR.assets.media, document.baseURI).href)
    .then((module) => module.mount())
    .catch((error) => console.error("aggr: media", error));
}

/* ------------------------------------------------------------------ preferences */

function installPreferences() {
  if (!PREFS) return;
  const status = (message) => {
    const node = $("#preferences-status");
    if (node) node.textContent = message;
  };

  function refreshThemeColor() {
    const meta = $("#theme-color");
    if (meta)
      meta.setAttribute(
        "content",
        getComputedStyle(document.documentElement).getPropertyValue("--nav-bg").trim(),
      );
  }

  /** Validate, apply to the document, and persist. Invalid values are ignored, never stored. */
  function apply(values, persist = true) {
    let saved = true;
    for (const [key, value] of Object.entries(values)) {
      if (!PREFS.valid(key, value)) continue;
      PREFS.values[key] = value;
      if (persist && !storage.write(localStorage, "aggr:" + key, String(value))) saved = false;
    }
    PREFS.apply(PREFS.values);
    refreshThemeColor();
    syncControls();
    dates.render();
    applyFeedPaging();
    return saved;
  }

  function syncControls() {
    for (const control of $$("[data-preference]")) {
      const key = control.dataset.preference || "";
      const value = PREFS.values[key];
      if (control instanceof HTMLInputElement && control.type === "checkbox")
        control.checked = Boolean(value);
      else if (control instanceof HTMLInputElement || control instanceof HTMLSelectElement)
        control.value = String(value);
    }
  }

  const payload = () => ({ version: 1, preferences: { ...PREFS.values } });

  function shareLink() {
    const encoded = btoa(JSON.stringify(payload()))
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/, "");
    const url = new URL("preferences/", BASE);
    url.hash = "aggr-state=" + encoded;
    return url.href;
  }

  /** @type {Record<string, unknown> | null} */
  let pending = null;

  function review(values) {
    pending = values;
    const section = $("#preferences-import");
    const list = $("#preferences-import-summary");
    if (!section || !list) return;
    list.replaceChildren(
      ...Object.entries(values).map(([key, value]) => {
        const item = document.createElement("li");
        const control = $("[data-preference='" + CSS.escape(key) + "']");
        const label = control?.closest(".setting")?.querySelector("strong, label")?.textContent;
        const option =
          control instanceof HTMLSelectElement
            ? Array.from(control.options).find((entry) => entry.value === String(value))?.textContent
            : null;
        const shown = typeof value === "boolean" ? (value ? "On" : "Off") : option || String(value);
        item.textContent = (label || key) + ": " + shown;
        return item;
      }),
    );
    section.hidden = false;
    status("Review the imported settings before applying them.");
  }

  function clearReview(message) {
    pending = null;
    const section = $("#preferences-import");
    if (section) section.hidden = true;
    if (message) status(message);
  }

  const parse = (raw) => {
    if (raw.length > 16384) throw new Error("Preferences file is too large");
    return PREFS.validate(JSON.parse(raw));
  };

  /** A shared link carries its payload in the fragment, so it never reaches a server log. */
  function importFragment() {
    const url = new URL(location.href);
    const encoded = new URLSearchParams(url.hash.slice(1)).get("aggr-state");
    if (!encoded) return;
    url.hash = "";
    history.replaceState(history.state, "", url.href);
    try {
      if (encoded.length > 22000 || !/^[A-Za-z0-9_-]+={0,2}$/.test(encoded))
        throw new Error("Invalid preferences link");
      let base64 = encoded.replace(/-/g, "+").replace(/_/g, "/");
      while (base64.length % 4) base64 += "=";
      const values = parse(atob(base64));
      if (KIND !== "preferences") {
        const destination = new URL("preferences/", BASE);
        destination.hash = "aggr-state=" + encoded;
        location.replace(destination.href);
        return;
      }
      review(values);
    } catch {
      clearReview("This preferences link is invalid or unsupported. Nothing was changed.");
    }
  }

  async function importFile(file) {
    try {
      if (file.size > 16384) throw new Error("File too large");
      review(parse(await file.text()));
    } catch {
      clearReview(
        "This file is invalid or unsupported. Use an aggr preferences JSON file (up to 16 KB). Nothing was changed.",
      );
    }
  }

  function offerLink(url) {
    const field = /** @type {HTMLInputElement | null} */ ($("#preferences-link"));
    if (!field) return;
    field.hidden = false;
    field.value = url;
    field.focus();
    field.select();
    status("Copy the selected link. Clipboard access is unavailable.");
  }

  const actions = {
    async copy() {
      const url = shareLink();
      try {
        if (!navigator.clipboard) return offerLink(url);
        await navigator.clipboard.writeText(url);
        status("Preferences link copied.");
      } catch {
        offerLink(url);
      }
    },
    async share() {
      try {
        await navigator.share({ title: "aggr preferences", url: shareLink() });
      } catch (error) {
        if (!(error instanceof Error && error.name === "AbortError"))
          status("Sharing is unavailable. Use Copy link or Save file.");
      }
    },
    save() {
      const blob = new Blob([JSON.stringify(payload(), null, 2) + "\n"], {
        type: "application/json",
      });
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = "aggr-preferences.json";
      document.body.appendChild(link);
      link.click();
      link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      status("Preferences file saved.");
    },
    import: () => $("#preferences-file")?.click(),
    apply() {
      if (!pending) return;
      const saved = apply(pending);
      clearReview(saved ? "Saved in this browser." : "Applied for this session; browser storage is unavailable.");
    },
    cancel: () => clearReview("Import cancelled. Your preferences were not changed."),
    reset() {
      const defaults = Object.fromEntries(
        Object.entries(PREFS.schema).map(([key, rule]) => [key, rule.initial]),
      );
      const saved = apply(defaults);
      clearReview(
        saved ? "Default preferences restored." : "Defaults applied for this session; browser storage is unavailable.",
      );
    },
  };

  document.addEventListener("change", (event) => {
    const control = event.target;
    if (!(control instanceof HTMLInputElement || control instanceof HTMLSelectElement)) return;
    if (control.id === "preferences-file") {
      const file = control.files?.[0];
      control.value = "";
      if (file) void importFile(file);
      return;
    }
    const key = control.dataset.preference;
    if (!key) return;
    if (!control.checkValidity()) {
      control.reportValidity();
      return;
    }
    const value =
      control instanceof HTMLInputElement && control.type === "checkbox"
        ? control.checked
        : control.type === "number"
          ? Number(control.value)
          : control.value;
    const saved = apply({ [key]: value });
    status(saved ? "Saved in this browser." : "Applied for this session; browser storage is unavailable.");
  });

  document.addEventListener("click", (event) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const button = target.closest("[data-preferences-action]");
    if (!(button instanceof HTMLElement)) return;
    event.preventDefault();
    const action = actions[button.dataset.preferencesAction || ""];
    if (action) void action();
  });

  // Another tab changed a setting: adopt it without reloading.
  window.addEventListener("storage", (event) => {
    if (event.key === null || Object.keys(PREFS.schema).some((key) => event.key === "aggr:" + key))
      apply(PREFS.read(), false);
  });

  const share = $("#share-state");
  if (share) share.hidden = typeof navigator.share !== "function";
  syncControls();
  refreshThemeColor();
  importFragment();
}

/* ------------------------------------------------------------------ new since last visit */

/**
 * Mark the entries that appeared since this browsing session last saw the feed. Without a
 * remembered head nothing is new, so a first visit never lights up the whole page.
 */
function markNewEntries() {
  if (KIND !== "river") return;
  const rows = selection.rows();
  if (!rows.length) return;
  const urls = rows.map((row) => row.dataset.url || "").filter(Boolean);
  const key = scopeKey("last-seen-entry");
  const head = storage.read(sessionStorage, key);
  if (head) {
    const boundary = urls.indexOf(head);
    const fresh = new Set(urls.slice(0, boundary === -1 ? urls.length : boundary));
    for (const row of rows) row.classList.toggle("is-new", fresh.has(row.dataset.url || ""));
  }
  if (urls[0]) storage.write(sessionStorage, key, urls[0]);
}

/* ------------------------------------------------------------------ service worker */

function registerWorker() {
  if (AGGR.pwa === false || !("serviceWorker" in navigator)) return;
  navigator.serviceWorker
    .register(new URL("sw.js", BASE).href, { scope: new URL(BASE).pathname, updateViaCache: "none" })
    .catch((error) => console.error("aggr: service worker", error));
}

/* ------------------------------------------------------------------ boot */

function boot() {
  safely("shift-hover", installShiftHover);
  safely("dates", () => dates.render());
  safely("selection", () => selection.restore());
  safely("feed-paging", applyFeedPaging);
  safely("new-entries", markNewEntries);
  safely("keyboard", installKeyboard);
  safely("shortcut-help", installShortcutHelp);
  safely("preferences", installPreferences);
  safely("search-intent", installSearchIntent);
  safely("media", loadMedia);
  safely("service-worker", registerWorker);

  // Keep relative times honest while the tab stays open.
  setInterval(() => {
    if (!document.hidden) dates.render();
  }, 60000);
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) dates.render();
  });
  window.addEventListener("pagehide", () => selection.save());

  // One readiness signal, for styles that only apply once enhancement is in place and for the
  // browser contract suite.
  document.documentElement.dataset.aggrReady = "true";
}

// A prerendered page runs before anyone has seen it: writing session state or history there would
// record visits that never happened.
if (document.prerendering) document.addEventListener("prerenderingchange", boot, { once: true });
else boot();

export { dates, selection, $, $$ };
