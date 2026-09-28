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
/** The kind of page on screen; moving to another page in place changes it. */
let KIND = AGGR.kind || document.body.dataset.kind || "";

/**
 * Everything a page sets up for itself (listeners on its own content, polling, players) belongs to
 * the page rather than the document: moving to another page in place ends it through this.
 */
let pageScope = new AbortController();

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
      store.setItem(key, value);
      return true;
    } catch {
      return false;
    }
  },
};

/** The element id a fragment names; one that is not valid percent-encoding names itself. */
function fragmentId(hash) {
  try {
    return decodeURIComponent(hash.slice(1));
  } catch {
    return hash.slice(1);
  }
}

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
    const days = Math.floor(hours / 24);
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

  return { render, format, text };
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
  // Search results are the feed filtered, so one cursor serves both. The static feed stays in the
  // document while a search is on screen and is hidden by its wrapper, not row by row: a row whose
  // container is hidden is not on screen and must not hold the cursor.
  const rows = () =>
    $$(".rows:not([aria-busy='true']) .row:not([hidden])").filter((row) => !row.closest("[hidden]"));
  const link = (row) => (row ? $("[data-row-open]", row) : null);

  function key(href = location.href) {
    const url = new URL(href);
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
  const write = (state, href) => storage.write(sessionStorage, key(href), JSON.stringify(state));

  // One cursor for the whole page: the list on screen may change under it, and a row left marked
  // in a list that is no longer shown would put a second cursor behind the first.
  const clear = () => {
    for (const entry of $$(".row.is-selected")) entry.classList.remove("is-selected");
  };

  function select(row, focus, scroll) {
    const target = link(row);
    if (!target) return false;
    clear();
    row.classList.add("is-selected");
    // The row is the whole cursor. Reading the scroll offset here would flush the layout the
    // classes above just invalidated, once per keystroke, for something nothing reads back.
    write({ url: target.href });
    if (focus) target.focus({ preventScroll: true });
    if (scroll) row.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "instant" });
    // The row under the cursor is the one Enter opens next.
    navigation.prefetch(target.href);
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
    /** Remember the cursor for the page at `href`, which is the one being left. */
    save(href = location.href) {
      const all = rows();
      if (!all.length) return;
      const selected = link(all.find((row) => row.classList.contains("is-selected")));
      if (selected) write({ url: selected.href }, href);
    },
    /** Reselect the remembered row, else the first one. Never steals focus on load. */
    restore() {
      const all = rows();
      if (!all.length) return;
      const wanted = read().url;
      const row = (wanted && all.find((entry) => link(entry)?.href === wanted)) || all[0];
      clear();
      row.classList.add("is-selected");
    },
    /** `focus: false` walks the list while the keyboard stays where it is, e.g. in the search field. */
    move(direction, focus = true) {
      const all = rows();
      if (!all.length) return false;
      const current = all.findIndex((row) => row.classList.contains("is-selected"));
      const next = current === -1 ? 0 : Math.min(all.length - 1, Math.max(0, current + direction));
      return select(all[next], focus, true);
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
  // A different slice of the list is a different set of rows: the cursor belongs on one of them.
  selection.restore();
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
  const routes = { f: "", i: "", b: "browse/", p: "preferences/" };
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
      // Held with Ctrl, d and u are the shortcut the help offers to anyone who turned the
      // single-key ones off; on their own they need those to be on.
      const byHalfPage = (event.key === "d" || event.key === "u") && (event.ctrlKey || single);
      if ((byLine || byHalfPage) && !dialog) {
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
        } else navigation.go(target.url);
      }
      return;
    }

    if (dialog) return;

    if (event.key === "?") {
      event.preventDefault();
      openShortcutHelp();
      return;
    }
    // Search from anywhere without a modifier. A page with no field of its own lands on the feed
    // with it focused, the same as Cmd/Ctrl+K.
    if (event.key === "/") {
      event.preventDefault();
      focusSearch();
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
    // The arrows walk a list wherever j and k would: nobody should have to know vim to read the
    // feed. An article page keeps them for scrolling, which is what reading it needs, and a list
    // with nothing in it leaves them to the browser.
    if (key === "ArrowDown" || key === "ArrowUp") {
      if (KIND !== "item" && selection.move(key === "ArrowDown" ? 1 : -1)) event.preventDefault();
      return;
    }
    // Case carries meaning from here on: the upper-case letters belong to the external links
    // below, so only the letter that was actually typed may claim one of these.
    if (event.key === "j" || event.key === "k") {
      const direction = event.key === "j" ? 1 : -1;
      if (KIND === "item") {
        const article = $("article.item");
        const url = direction === 1 ? article?.dataset.nextUrl : article?.dataset.previousUrl;
        event.preventDefault();
        // Stepping past either end of the archive returns to the feed rather than stopping dead.
        navigation.go(new URL(url || "", BASE).href);
        return;
      }
      if (selection.move(direction)) event.preventDefault();
      return;
    }
    if ((event.key === "o" || event.key === "Enter") && KIND !== "item") {
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

/**
 * Whether the search field sits entirely between the sticky header and the bottom bar. Moving the
 * page when the reader can already see the field would cost them their place for nothing.
 */
function searchFieldVisible(field) {
  const box = field.getBoundingClientRect();
  const header = $(".top")?.getBoundingClientRect().bottom ?? 0;
  const tabs = $(".mobile-tabs")?.getBoundingClientRect();
  const floor = tabs && tabs.height > 0 ? tabs.top : window.innerHeight;
  return box.top >= header && box.bottom <= floor;
}

/** Bring the search field into view, but only when it is not already there. */
function revealSearchField(field) {
  if (!searchFieldVisible(field)) window.scrollTo({ top: 0, behavior: "instant" });
}

/** Focus the shared search field, coming home first when the current page has none. */
function focusSearch() {
  const field = /** @type {HTMLInputElement | null} */ ($("#q"));
  if (!field) {
    navigation.go(new URL("?focus-search=1", BASE).href);
    return;
  }
  loadSearch();
  // Focus first without moving, then decide: the field's own box is what we measure.
  field.focus({ preventScroll: true });
  revealSearchField(field);
  field.select();
}

let searchModule;
/** Each page's search controls, mounted once. */
const searchMounts = new WeakMap();
/** The search engine is a separate file, fetched with the page that can use it. */
function loadSearch() {
  const root = $("[data-search-root]");
  if (!AGGR.assets?.search || !root) return undefined;
  if (searchMounts.has(root)) return searchMounts.get(root);
  const signal = pageScope.signal;
  searchModule ??= import(new URL(AGGR.assets.search, document.baseURI).href);
  const mounted = searchModule
    .then((module) => {
      if (signal.aborted || !root.isConnected) return;
      module.mount({ base: BASE, dates, selection, preferences: PREFS, signal, navigate: navigation.go });
    })
    .catch((error) => console.error("aggr: search", error));
  searchMounts.set(root, mounted);
  return mounted;
}

function installSearchIntent() {
  const toolbar = $(".feed-toolbar");
  if (!toolbar) return;
  const field = /** @type {HTMLInputElement | null} */ ($("#q"));
  if (field) {
    for (const name of ["click", "focus"])
      field.addEventListener(name, () => revealSearchField(field));
    // Any sign of typing loads the engine, not just pointer or focus intent: a keystroke that
    // arrives before the module would otherwise be dropped. search.js runs whatever it finds in
    // the field once it mounts.
    for (const name of ["keydown", "input"])
      field.addEventListener(name, () => loadSearch(), { capture: true });
  }
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

/* ------------------------------------------------------------------ navigation */

/**
 * Following a link inside the archive swaps the page in place instead of loading a new document:
 * the header, the tab bar and the scripts already running stay, and a page fetched while the finger
 * was still coming down is on screen in the frame after it lifts. Links stay ordinary links, so
 * modifier clicks, new tabs and readers without JavaScript get plain navigation, and anything this
 * cannot swap (another site, a file, an error page, no network) falls back to it.
 *
 * Scroll positions are this module's to keep: the browser would restore them before the page they
 * belong to had arrived.
 */
const navigation = (() => {
  const root = new URL(BASE);
  /** Fetched pages by address, least recently used first. */
  const pages = new Map();
  const PAGE_LIMIT = 24;
  /** A page fetched longer ago than this is fetched again rather than shown. */
  const PAGE_LIFETIME = 5 * 60000;
  /** Pages one screen may fetch before anyone asks for them. */
  const SPECULATION_LIMIT = 16;
  /** Where each history entry was scrolled to, by the key its state carries. */
  const scrolls = new Map();
  /**
   * Where each list was last left, by address. Like a native tab, a list reached again through
   * its tab picks up where the reader was; asking for the list already on screen goes to its top.
   */
  const places = new Map();
  const PLACE_LIMIT = 50;
  let current = address(location.href);
  let entry = "";
  let token = 0;
  /** Whether a page is on its way: until it lands, the one on screen is no longer current. */
  let pending = false;
  let speculated = 0;
  let installed = false;

  /** A page's address without its fragment: two fragments of one page are one page. */
  function address(href) {
    const url = new URL(href, location.href);
    url.hash = "";
    return url.href;
  }

  const newKey = () => Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
  const stateWith = (fields) => ({ ...(history.state || {}), aggr: { key: entry, ...fields } });

  /** Pages of this archive, as opposed to its files (feeds, Markdown, TOML) and other sites. */
  const routable = (url) =>
    url.origin === root.origin &&
    url.pathname.startsWith(root.pathname) &&
    (url.pathname.endsWith("/") || url.pathname.endsWith(".html"));

  function linkIn(event) {
    const target = event.target;
    const link = target instanceof Element ? target.closest("a[href]") : null;
    if (!(link instanceof HTMLAnchorElement)) return null;
    if (link.hasAttribute("download") || (link.target && link.target !== "_self")) return null;
    if (link.relList.contains("external")) return null;
    const url = new URL(link.href);
    return routable(url) ? { link, url } : null;
  }

  /** A reader who asked the browser to save data, or who is on a slow link, gets no guesses. */
  const frugal = () => {
    const connection = /** @type {any} */ (navigator).connection;
    return Boolean(connection?.saveData) || /2g$/.test(connection?.effectiveType || "");
  };

  const idle = (run) =>
    "requestIdleCallback" in window ? requestIdleCallback(run, { timeout: 1500 }) : setTimeout(run, 250);

  const parse = (html) => new DOMParser().parseFromString(html, "text/html");

  /** A request for the page at `key`, remembered once it has arrived. */
  function request(key) {
    const record = { time: Date.now(), page: Promise.resolve(null), parsed: null, ready: false };
    record.page = fetch(key, { headers: { accept: "text/html" }, credentials: "same-origin" }).then(
      async (response) => {
        const type = response.headers.get("content-type") || "";
        if (!response.ok || !type.includes("text/html")) throw new Error("not a page");
        const page = { url: response.url || key, html: await response.text() };
        record.ready = true;
        // Parsing is the one step left between the tap and the swap; do it while nothing waits.
        idle(() => {
          if (pages.get(key) === record && !record.parsed) record.parsed = parse(page.html);
        });
        return page;
      },
    );
    return record;
  }

  function remember(key, record) {
    pages.delete(key);
    pages.set(key, record);
    while (pages.size > PAGE_LIMIT) pages.delete(pages.keys().next().value);
  }

  /**
   * The page at `href`: from memory while it is fresh or still on its way, else from the network.
   * A page fetched longer ago than PAGE_LIFETIME may belong to an earlier build or release, so it
   * is fetched again, and its old copy stands in only when the network cannot answer. The pages a
   * screen links to most are kept fresh while it is open (see `speculate`), so this rarely waits.
   */
  function load(href) {
    const key = address(href);
    const cached = pages.get(key);
    if (cached && !(cached.ready && Date.now() - cached.time >= PAGE_LIFETIME)) {
      remember(key, cached);
      return cached;
    }
    const record = request(key);
    if (cached) {
      record.page = record.page.catch(() => {
        record.time = cached.time;
        record.ready = true;
        return cached.page;
      });
    }
    record.page.catch(() => {
      if (pages.get(key) === record) pages.delete(key);
    });
    remember(key, record);
    return record;
  }

  /** Fetch a page the reader is about to open. Guesses are counted and bounded; intent is not. */
  function prefetch(href, guess = false) {
    let url;
    try {
      url = new URL(href, location.href);
    } catch {
      return;
    }
    const key = address(url.href);
    if (!routable(url) || key === current) return;
    if (guess) {
      if (frugal() || (speculated >= SPECULATION_LIMIT && !pages.has(key))) return;
      if (!pages.has(key)) speculated += 1;
    }
    load(url.href).page.catch(() => {});
  }

  /** Leave the page on screen: keep its place and its cursor, and end what it set up. */
  function leave() {
    if (pageScope.signal.aborted) return;
    scrolls.set(entry, window.scrollY);
    if (KIND !== "item") {
      places.delete(current);
      places.set(current, window.scrollY);
      while (places.size > PLACE_LIMIT) places.delete(places.keys().next().value);
    }
    selection.save(current);
    pageScope.abort();
  }

  /** Bring the persistent parts of the page (header, tab bar) in line with the page arriving. */
  function syncRegion(selector, next) {
    const region = $(selector);
    const fresh = next.querySelector(selector);
    if (!region || !fresh) return;
    const old = $$("a, img", region);
    const renewed = Array.from(fresh.querySelectorAll("a, img"));
    if (old.length !== renewed.length) {
      region.replaceWith(document.adoptNode(fresh));
      return;
    }
    old.forEach((node, index) => {
      for (const name of ["href", "src", "aria-current"]) {
        const value = renewed[index].getAttribute(name);
        if (value === null) node.removeAttribute(name);
        else if (node.getAttribute(name) !== value) node.setAttribute(name, value);
      }
    });
  }

  /** Swap the page's own metadata; scripts, styles and settings shared by every page stay. */
  function syncHead(next) {
    const shared = (node) =>
      node.matches(
        'title, script:not([type="application/ld+json"]), style, link[rel~="stylesheet"], meta[charset], meta[name="viewport"], #theme-color',
      );
    const old = Array.from(document.head.children).filter((node) => !shared(node));
    const renewed = Array.from(next.head.children).filter((node) => !shared(node));
    const kept = new Set(renewed.map((node) => node.outerHTML));
    const present = new Set(old.map((node) => node.outerHTML));
    for (const node of old) if (!kept.has(node.outerHTML)) node.remove();
    for (const node of renewed)
      if (!present.has(node.outerHTML)) document.head.appendChild(document.adoptNode(node));
    const data = $("#aggr-page");
    const incoming = next.getElementById("aggr-page");
    if (data && incoming) data.textContent = incoming.textContent;
  }

  function swap(next, main, context) {
    document.title = next.title;
    syncHead(next);
    for (const key of Object.keys(AGGR)) if (!(key in context)) delete AGGR[key];
    Object.assign(AGGR, context);
    KIND = AGGR.kind || next.body.dataset.kind || "";
    const body = document.body;
    for (const name of body.getAttributeNames())
      if (!next.body.hasAttribute(name)) body.removeAttribute(name);
    for (const name of next.body.getAttributeNames())
      body.setAttribute(name, next.body.getAttribute(name) || "");
    syncRegion(".top", next);
    syncRegion(".mobile-tabs", next);
    $("#content")?.replaceWith(document.adoptNode(main));
  }

  /** When the page last moved, to tell a page still gliding from one at rest. */
  let scrolled = -Infinity;

  /** Put the reader where the page they arrived at expects them, and the keyboard with it. */
  function arrive(saved, hash) {
    const id = hash ? fragmentId(hash) : "";
    const target = id ? document.getElementById(id) : null;
    const place = () => {
      if (saved !== undefined) window.scrollTo({ top: saved, behavior: "instant" });
      else if (target) target.scrollIntoView({ block: "start", behavior: "instant" });
      else window.scrollTo({ top: 0, behavior: "instant" });
    };
    // A fling or a trackpad's momentum outlives the page it moved and would drag the next one
    // along. Taking the scrollbar away through one painted frame is what makes the compositor let
    // go of it, and until then any step it still takes is undone before it is painted. A page at
    // rest is left alone, since a scrollbar's width could shift that frame.
    const gliding = performance.now() - scrolled < 150;
    const root = document.documentElement;
    place();
    if (gliding) {
      root.style.overflow = "hidden";
      window.addEventListener("scroll", place, { passive: true });
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          root.style.removeProperty("overflow");
          window.removeEventListener("scroll", place);
          place();
        }),
      );
    }
    const main = $("#content");
    if (main) {
      main.setAttribute("data-navigation-focus", "");
      main.focus({ preventScroll: true });
    }
  }

  /** The stylesheets and scripts a document runs on, resolved against its own address. */
  const shell = (doc, base) =>
    Array.from(doc.querySelectorAll('link[rel~="stylesheet"][href], script[src]'))
      .map((node) => new URL(node.getAttribute("href") || node.getAttribute("src") || "", base).href)
      .sort()
      .join(" ");

  /** The shell this document loaded with, read while its address still matches its links. */
  let runningShell = "";

  /** Whatever this cannot swap is still a link: let the browser follow it. */
  function fallback(href, traverse) {
    if (traverse) location.reload();
    else location.assign(href);
  }

  let slow = 0;
  /** Say a page is on its way once it has taken longer than a glance, and stop saying so. */
  function waiting(on) {
    clearTimeout(slow);
    if (on) slow = setTimeout(() => document.documentElement.setAttribute("data-navigating", ""), 150);
    else document.documentElement.removeAttribute("data-navigating");
  }

  /** The row or card whose link is on its way, lit until its page replaces it. */
  let opening = null;

  /**
   * Answer the tap before the page arrives: the tab or menu entry chosen becomes current, and the
   * row or card followed stays lit.
   */
  function acknowledge(link) {
    opening?.removeAttribute("data-opening");
    opening = link?.closest(".row, .article-more-card") || null;
    opening?.setAttribute("data-opening", "");
    if (!link?.closest("[data-site-navigation]") || !link.hasAttribute("data-route")) return;
    const route = link.getAttribute("data-route");
    for (const entry of $$("[data-site-navigation] a[data-kinds]")) {
      if (entry.getAttribute("data-route") === route) entry.setAttribute("aria-current", "page");
      else entry.removeAttribute("aria-current");
    }
  }

  /** Take an answer back: the page on screen is the current one again. */
  function unacknowledge() {
    opening?.removeAttribute("data-opening");
    opening = null;
    for (const entry of $$("[data-site-navigation] a[data-kinds]")) {
      if ((entry.dataset.kinds || "").split(" ").includes(KIND)) entry.setAttribute("aria-current", "page");
      else entry.removeAttribute("aria-current");
    }
  }

  /** The reader chose to stay: a page still on its way must not replace this one when it lands. */
  function cancel() {
    if (pending) unacknowledge();
    token += 1;
    pending = false;
    waiting(false);
  }

  /** The page on screen, asked for again: its top, with nothing left mid-edit. */
  function top() {
    cancel();
    const active = document.activeElement;
    if (active instanceof HTMLElement && editing(active)) active.blur();
    window.scrollTo({ top: 0, behavior: "instant" });
  }

  /**
   * Show the page at `href`. `replace` stands in for the current history entry; `traverse` means
   * Back or Forward already moved the address bar, and the page has to catch up with it.
   */
  async function go(href, { replace = false, traverse = false, from = null } = {}) {
    const url = new URL(href, location.href);
    if (!installed || !routable(url)) {
      if (replace) location.replace(url.href);
      else location.assign(url.href);
      return;
    }
    if (!traverse && address(url.href) === current) {
      if (url.hash) location.hash = url.hash;
      else top();
      return;
    }
    const mine = ++token;
    pending = true;
    waiting(true);
    acknowledge(from);
    const record = load(url.href);
    const page = await record.page.catch(() => null);
    if (mine !== token) return;
    pending = false;
    waiting(false);
    if (!page) return fallback(url.href, traverse);
    const next = record.parsed || parse(page.html);
    // The parsed page is taken apart by the swap; the next visit parses its own copy.
    record.parsed = null;
    const main = next.getElementById("content");
    let context;
    try {
      context = JSON.parse(next.getElementById("aggr-page")?.textContent || "null");
    } catch {
      context = null;
    }
    if (!main || !context) return fallback(url.href, traverse);
    // A page from a newer release needs that release's styles and scripts: load it whole.
    if (shell(next, page.url) !== runningShell) return fallback(url.href, traverse);

    if (!traverse) {
      leave();
      const destination = new URL(page.url);
      destination.hash = url.hash;
      entry = newKey();
      if (replace) history.replaceState({ aggr: { key: entry } }, "", destination.href);
      else history.pushState({ aggr: { key: entry } }, "", destination.href);
    }
    current = address(location.href);
    swap(next, main, context);
    opening = null;
    pageScope = new AbortController();
    const saved = traverse
      ? (scrolls.get(entry) ?? history.state?.aggr?.scroll)
      : KIND !== "item" && !url.hash
        ? places.get(current)
        : undefined;
    arrive(saved, url.hash);
    mountPage();
  }

  /** Record a new entry for a place in this page, as following a fragment link would. */
  function pushFragment(hash) {
    scrolls.set(entry, window.scrollY);
    entry = newKey();
    history.pushState({ aggr: { key: entry } }, "", hash);
  }

  /**
   * A tab answers the touch that lands on it, including one that lands while the page is still
   * gliding from a fling: the platform spends that touch on stopping the scroll and never sends
   * the click, so the release is the activation, on whichever tab it lifts from.
   */
  function installTabActivation() {
    const tabs = $(".mobile-tabs");
    if (!tabs) return;
    let pressed = null;
    tabs.addEventListener(
      "touchstart",
      (event) => {
        const touch = event.touches.length === 1 ? event.touches[0] : null;
        pressed = touch ? { y: touch.clientY } : null;
        const target = event.target;
        const link = target instanceof Element ? target.closest("a[href]") : null;
        if (link instanceof HTMLAnchorElement) prefetch(link.href);
      },
      { passive: true },
    );
    // A finger sliding along the bar still means the tab it lifts from, as on a native tab bar;
    // one travelling up or down is scrolling the page instead.
    tabs.addEventListener(
      "touchmove",
      (event) => {
        const touch = event.touches[0];
        if (pressed && touch && Math.abs(touch.clientY - pressed.y) > 24) pressed = null;
      },
      { passive: true },
    );
    tabs.addEventListener("touchcancel", () => (pressed = null), { passive: true });
    tabs.addEventListener(
      "touchend",
      (event) => {
        const touched = pressed;
        pressed = null;
        if (!touched || event.touches.length) return;
        const touch = event.changedTouches[0];
        const under = touch ? document.elementFromPoint(touch.clientX, touch.clientY) : null;
        const link = under?.closest("a[href]");
        if (!(link instanceof HTMLAnchorElement) || !tabs.contains(link)) return;
        // Cancelling the release cancels the click the platform would have sent after it, so
        // one tap stays one navigation.
        event.preventDefault();
        link.click();
      },
      { passive: false },
    );
  }

  /** The tabs and the neighbouring articles: where a reader is most likely to go from here. */
  function nearest() {
    const article = $("article.item");
    return [
      ...$$("[data-site-navigation] a[data-route]").map(
        (link) => /** @type {HTMLAnchorElement} */ (link).href,
      ),
      ...[article?.dataset.nextUrl, article?.dataset.previousUrl]
        .filter(Boolean)
        .map((neighbour) => new URL(neighbour || "", BASE).href),
    ];
  }

  /** Fetch what this page makes likely next, once the page itself has settled. */
  function speculate() {
    speculated = 0;
    const signal = pageScope.signal;
    // A screen left open, or an app brought back, keeps its nearest pages fresh, so following one
    // of them never waits on a copy that has gone stale.
    const refresh = () => {
      if (!document.hidden && !frugal()) for (const href of nearest()) prefetch(href);
    };
    const timer = setInterval(refresh, PAGE_LIFETIME / 2);
    document.addEventListener("visibilitychange", refresh, { signal });
    signal.addEventListener("abort", () => clearInterval(timer));
    idle(() => {
      if (signal.aborted || frugal()) return;
      for (const href of nearest()) prefetch(href, true);
      if (!("IntersectionObserver" in window)) return;
      // A row the reader lingers over, rather than every row that scrolls past.
      const timers = new Map();
      const observer = new IntersectionObserver((entries) => {
        for (const seen of entries) {
          const link = /** @type {HTMLAnchorElement} */ (seen.target);
          clearTimeout(timers.get(link));
          timers.delete(link);
          if (!seen.isIntersecting) continue;
          timers.set(
            link,
            setTimeout(() => {
              observer.unobserve(link);
              prefetch(link.href, true);
            }, 250),
          );
        }
      });
      for (const link of $$(".rows .row [data-row-open], .article-more-link")) observer.observe(link);
      signal.addEventListener("abort", () => {
        observer.disconnect();
        timers.forEach((timer) => clearTimeout(timer));
      });
    });
  }

  function install() {
    if (!("pushState" in history) || typeof DOMParser !== "function") return;
    installed = true;
    runningShell = shell(document, location.href);
    history.scrollRestoration = "manual";
    window.addEventListener("scroll", () => (scrolled = performance.now()), { passive: true });
    entry = history.state?.aggr?.key || newKey();
    const saved = history.state?.aggr?.scroll;
    history.replaceState(stateWith({}), "");
    // A reload or a return to a document the browser let go of: its own place, as it was left.
    const arrival = /** @type {PerformanceNavigationTiming | undefined} */ (
      performance.getEntriesByType?.("navigation")?.[0]
    );
    if (typeof saved === "number" && (arrival?.type === "reload" || arrival?.type === "back_forward"))
      requestAnimationFrame(() => window.scrollTo({ top: saved, behavior: "instant" }));

    document.addEventListener("click", (event) => {
      if (event.defaultPrevented || event.button !== 0) return;
      if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const hit = linkIn(event);
      if (!hit) return;
      if (address(hit.url.href) === current) {
        // A fragment of this page is the browser's own jump; the entry it leaves keeps its place.
        if (hit.url.hash) {
          cancel();
          scrolls.set(entry, window.scrollY);
          return;
        }
        event.preventDefault();
        top();
        return;
      }
      event.preventDefault();
      void go(hit.url.href, { from: hit.link });
    });
    // Intent: a press is a promise of a click, and a pointer resting on a link is a likely one.
    document.addEventListener(
      "pointerdown",
      (event) => {
        const hit = linkIn(event);
        if (hit) prefetch(hit.url.href);
      },
      { capture: true, passive: true },
    );
    let hover;
    document.addEventListener(
      "pointerover",
      (event) => {
        clearTimeout(hover);
        if (event.pointerType !== "mouse") return;
        const hit = linkIn(event);
        if (hit) hover = setTimeout(() => prefetch(hit.url.href), 60);
      },
      { passive: true },
    );
    document.addEventListener("pointerout", () => clearTimeout(hover), { passive: true });

    window.addEventListener("popstate", (event) => {
      const key = event.state?.aggr?.key;
      if (address(location.href) === current && !pending) {
        // Another place in this same page. Nothing was fetched, so nothing needs to arrive.
        scrolls.set(entry, window.scrollY);
        entry = key || newKey();
        const saved = scrolls.get(entry);
        if (saved !== undefined) window.scrollTo({ top: saved, behavior: "instant" });
        return;
      }
      leave();
      entry = key || newKey();
      if (!key) history.replaceState(stateWith({}), "");
      void go(location.href, { traverse: true });
    });
    // A fragment the browser followed by itself made an entry with no key: give it one.
    window.addEventListener("hashchange", () => {
      if (history.state?.aggr?.key) return;
      entry = newKey();
      history.replaceState(stateWith({}), "");
    });
    // The place this entry was left at, for a reload or a Back that has to rebuild the document.
    window.addEventListener("pagehide", () => {
      try {
        history.replaceState(stateWith({ scroll: window.scrollY }), "");
      } catch {
        /* a browser rationing history writes may refuse this one */
      }
    });
    installTabActivation();
  }

  return { install, go, prefetch, pushFragment, speculate };
})();

/* ------------------------------------------------------------------ tap feedback */

/**
 * Touch has no hover to fall back on, and `:active` waits to find out whether a touch was really
 * a scroll. A tab says it was hit the moment it is touched; the stylesheet does the rest.
 */
function installTapFeedback() {
  const tabs = $(".mobile-tabs");
  if (!tabs) return;
  const clear = () => {
    for (const pressed of $$("[data-pressed]", tabs)) pressed.removeAttribute("data-pressed");
  };
  tabs.addEventListener(
    "pointerdown",
    (event) => {
      const target = event.target;
      const link = target instanceof Element ? target.closest("a") : null;
      clear();
      if (link) link.setAttribute("data-pressed", "");
    },
    { passive: true },
  );
  for (const name of ["pointerup", "pointercancel", "pointerleave"])
    tabs.addEventListener(name, clear, { passive: true });
  window.addEventListener("pagehide", clear);
  // WebKit only shows `:active` on touch when a page listens for touches at all.
  document.addEventListener("touchstart", () => {}, { passive: true });
}

/* ------------------------------------------------------------------ pictures */

/**
 * Every picture is painted over the placeholder the build inlined behind it, and falls back to its
 * alt text when it never arrives. Two capturing listeners cover the whole document, including the
 * rows search adds later, so no page has to load a module to show a picture honestly.
 */
const pictureFrame = (image) =>
  image.closest(".article-picture, .article-lead, .preview-media, .audio-cover, .media-frame");
// A picture that arrives after a failure, or fails after arriving, must not keep both marks:
// the same source is retried whenever a reader comes back to a page that had no network.
const markPicture = (box, loaded) => {
  box.classList.toggle("is-loaded", loaded);
  box.classList.toggle("is-error", !loaded);
};

function installPictureStates() {
  const settle = (event, loaded) => {
    const target = event.target;
    if (!(target instanceof HTMLImageElement)) return;
    const box = pictureFrame(target);
    if (box) markPicture(box, loaded);
  };
  document.addEventListener("load", (event) => settle(event, true), true);
  document.addEventListener("error", (event) => settle(event, false), true);
}

/** A picture the browser had already finished with never fires either event. */
function settlePictures() {
  for (const image of $$("img")) {
    if (!(image instanceof HTMLImageElement) || !image.complete) continue;
    const box = pictureFrame(image);
    if (box) markPicture(box, image.naturalWidth > 0);
  }
}

/* ------------------------------------------------------------------ on-screen keyboard */

/**
 * The on-screen keyboard shrinks the visual viewport, but the floating tab bar belongs to the
 * layout viewport: left alone it sits on top of the keyboard, over the results someone is typing
 * to find. It steps aside while an editor holds the keyboard, and the page itself does not move.
 */
function installKeyboardInset() {
  const viewport = window.visualViewport;
  if (!viewport || !$(".mobile-tabs")) return;
  const sync = () =>
    document.documentElement.classList.toggle(
      "is-keyboard-open",
      window.innerHeight - viewport.height > 120 && editing(document.activeElement),
    );
  viewport.addEventListener("resize", sync, { passive: true });
  for (const name of ["focusin", "focusout"])
    document.addEventListener(name, sync, { passive: true });
}

/* ------------------------------------------------------------------ media */

/** Players and their timing readouts only exist on article pages that carry media, where they
 * load with the page rather than when someone reaches for the controls. */
function loadMedia() {
  if (!AGGR.assets?.media) return;
  if (!$(".video-player, [data-audio-component], .native-video, [data-media-timing]")) return;
  const signal = pageScope.signal;
  import(new URL(AGGR.assets.media, document.baseURI).href)
    .then((module) => signal.aborted || module.mount({ signal }))
    .catch((error) => console.error("aggr: media", error));
}

/* ------------------------------------------------------------------ reading header */

/**
 * Reading progress and the folding header are scroll-driven animations in the stylesheet. Where
 * the browser has no scroll timelines, the bar follows the scroll from here, on its own transform.
 * Where it has them, the compact title the header folds into is measured once per resize, and so
 * are the header's natural heights where `calc-size()` is missing (Safari).
 */
function installReadingHeader() {
  const header = $("body[data-kind='item'] .itemhead");
  if (!header) return;
  const signal = pageScope.signal;
  if (!CSS.supports("animation-timeline: scroll()")) {
    const bar = $(".itemhead-progress", header);
    if (!bar) return;
    let frame = 0;
    const paint = () => {
      frame = 0;
      const range = document.documentElement.scrollHeight - window.innerHeight;
      const progress = range > 0 ? Math.min(1, Math.max(0, window.scrollY / range)) : 0;
      bar.style.transform = "scaleX(" + progress + ")";
    };
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(paint);
    };
    window.addEventListener("scroll", schedule, { passive: true, signal });
    window.addEventListener("resize", schedule, { passive: true, signal });
    paint();
    return;
  }
  if (!("ResizeObserver" in window)) return;
  // The compact title the header folds into, set across the full width: its height is the one the
  // box closes to. Without calc-size() the natural heights are measured as well.
  const measured = [[$(".itemhead-title-compact", header), "--compact-height"]];
  if (!CSS.supports("height: calc-size(auto, size)"))
    measured.push(
      [$(".itemhead-title", header), "--title-height"],
      [$(".item-tags-inner", header), "--tags-height"],
    );
  // The untransformed border box: the title is scaled, and whole pixels would jolt the fold.
  const observer = new ResizeObserver((entries) => {
    for (const entry of entries) {
      const property = measured.find(([node]) => node === entry.target)?.[1];
      const height = entry.borderBoxSize?.[0]?.blockSize ?? entry.contentRect.height;
      if (property) header.style.setProperty(property, height + "px");
    }
  });
  for (const [node] of measured) if (node) observer.observe(node);
  signal.addEventListener("abort", () => observer.disconnect());
}

/* ------------------------------------------------------------------ preferences */

/**
 * Light up both halves of a footnote when either one is followed. `:target` already covers the
 * half the fragment names; this marks the other, which is the margin note beside the reference
 * when the notes list is off screen, and the reference itself when the note points back to it.
 */
function installFootnoteTargets() {
  const paired = () => {
    for (const marked of $$("[data-footnote-active]")) marked.removeAttribute("data-footnote-active");
    const id = fragmentId(location.hash);
    if (!id) return;
    const note = document.getElementById(id);
    if (!note) return;
    // A note: mark it, and the margin note that stands in for it beside each reference.
    if (note.closest(".footnotes")) {
      note.setAttribute("data-footnote-active", "");
      for (const reference of $$('.footnote-ref a[href="#' + CSS.escape(id) + '"]')) {
        const aside = reference.parentElement?.nextElementSibling;
        if (aside?.classList.contains("footnote-margin-note")) {
          aside.setAttribute("data-footnote-active", "");
        }
      }
      return;
    }
    // A reference: mark the marker, so the word it sits against is easy to find again.
    const reference = note.closest(".footnote-ref, .citation-ref");
    if (reference) reference.setAttribute("data-footnote-active", "");
  };
  paired();
  window.addEventListener("hashchange", paired, { signal: pageScope.signal });
}

/**
 * A heading is a place in the article, so the whole of it is the way back to that place, not only
 * the `#` beside it. The heading stays a heading rather than becoming a link: the reader's own
 * selection, and any link the publisher wrote inside it, come first.
 *
 * The sticky stack is measured at the moment of the jump instead of being tracked. The header
 * folds as the page scrolls, so its height is only knowable then, and a jump from the top lands a
 * little low rather than under a header that is about to shrink.
 */
function installHeadingAnchors() {
  const body = $(".body");
  if (!body) return;

  const clearance = () =>
    [$(".top"), $(".itemhead")].reduce(
      (total, element) => total + (element?.getBoundingClientRect().height || 0),
      16,
    );

  const jump = (id, record) => {
    const target = document.getElementById(id);
    if (!target) return;
    if (record && location.hash.slice(1) !== id) navigation.pushFragment("#" + encodeURIComponent(id));
    const top = window.scrollY + target.getBoundingClientRect().top - clearance();
    window.scrollTo({ top: Math.max(0, top), behavior: "instant" });
  };

  body.addEventListener("click", (event) => {
    if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    const target = event.target;
    if (!(target instanceof Element)) return;
    const heading = target.closest(".body :is(h1, h2, h3, h4, h5, h6)[id]");
    if (!heading) return;
    // Anything the publisher made clickable keeps its own behaviour; only the `#` is ours.
    const link = target.closest("a");
    if (link && !link.classList.contains("heading-anchor")) return;
    if (!getSelection()?.isCollapsed) return;
    event.preventDefault();
    jump(heading.id, true);
  });

  // The browser scrolls a fragment into view against `scroll-padding-top`, which cannot know how
  // tall the folding header is without measuring it on every frame. Correct it once the page has
  // settled, and on every later jump.
  const settle = () => {
    const id = fragmentId(location.hash);
    if (id && document.getElementById(id)?.closest(".body")) jump(id, false);
  };
  window.addEventListener("hashchange", settle, { signal: pageScope.signal });
  requestAnimationFrame(settle);
}

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
        navigation.go(destination.href, { replace: true });
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

  preparePreferences = () => {
    const share = $("#share-state");
    if (share) share.hidden = typeof navigator.share !== "function";
    // The controls ship disabled so they cannot take a value nothing would save.
    const controls = /** @type {HTMLFieldSetElement | null} */ ($("#preferences-controls"));
    if (controls) controls.disabled = false;
    syncControls();
    refreshThemeColor();
    importFragment();
  };
}

/** The part of the preferences that belongs to the page on screen, set once they are installed. */
let preparePreferences = () => {};

/* ------------------------------------------------------------------ new since last visit */

/**
 * Mark the entries that appeared since this browsing session last saw the feed. Without a
 * remembered head nothing is new, so a first visit never lights up the whole page.
 */
/** How often a page that shows a list asks whether the build has moved on. */
const UPDATE_INTERVAL = 300000;

/**
 * New items should arrive without anyone reaching for reload. `updates.json` is the build's own
 * statement of what it published, so a list page polls it, and swaps its rows for the current
 * ones when the content version has moved. The swap waits for a moment that does not move the
 * ground under the reader: the top of the list, or coming back to the window.
 */
function installFeedUpdates() {
  if (!AGGR.content || !$(".rows:not(.search-results)")) return;
  const signal = pageScope.signal;
  let content = AGGR.content;
  let wanted = null;
  let entries = null;
  let pending = false;
  let swapping = false;

  async function swap() {
    if (swapping || !wanted) return;
    swapping = true;
    // The build this fetch is for. A check that finishes mid-flight moves `wanted` on, and
    // recording that newer version against these rows would mark it applied and leave the reader
    // stale until some third build arrived. Only the version is pinned: the page comes back as
    // whatever is live when it is fetched, so its shortcuts are the newest ones known, not the
    // ones that were current when this pass started.
    const target = wanted;
    try {
      const response = await fetch(location.href, { cache: "no-store", signal });
      if (!response.ok) return;
      const page = new DOMParser().parseFromString(await response.text(), "text/html");
      // The reader may have moved on to another page while this was in flight.
      if (signal.aborted) return;
      const rows = $(".rows:not(.search-results)");
      const fresh = page.querySelector(".rows:not(.search-results)");
      if (!rows || !fresh) return;
      rows.replaceWith(document.importNode(fresh, true));
      const pager = page.querySelector("[data-feed-pager]");
      const current = $("[data-feed-pager]");
      if (pager && current) current.replaceWith(document.importNode(pager, true));
      // The numbered shortcuts point at the newest entries, which are the ones that just changed.
      if (Array.isArray(entries)) AGGR.entries = entries;
      dates.render();
      applyFeedPaging();
      markNewEntries();
      selection.restore();
      // Only now are the rows on screen this build's. Recording the version before the swap
      // landed meant one failed refresh made every later check believe it was already applied,
      // and the reader kept stale rows until some other build shipped.
      content = target;
      // Anything newer that arrived while this was in flight is still owed a pass.
      if (wanted === target) wanted = null;
      else pending = true;
    } catch {
      // Offline, aborted, or a broken response: `content` stays where it was, so the next check
      // reports the same new version and asks for it again.
    } finally {
      swapping = false;
    }
  }

  const settle = (activated = false) => {
    if (!pending || (!activated && window.scrollY > 200)) return;
    pending = false;
    void swap();
  };

  const check = async (activated = false) => {
    try {
      const response = await fetch(new URL("updates.json", BASE).href, { cache: "no-store" });
      if (!response.ok) return;
      const update = await response.json();
      if (typeof update.content_version !== "string" || update.content_version === content) return;
      wanted = update.content_version;
      entries = Array.isArray(update.entries) ? update.entries : null;
      pending = true;
    } catch {
      return;
    }
    settle(activated);
  };

  void check();
  const timer = setInterval(() => {
    if (!document.hidden) void check();
  }, UPDATE_INTERVAL);
  signal.addEventListener("abort", () => clearInterval(timer));
  // Opening the app again is the moment a reader expects to be caught up.
  document.addEventListener(
    "visibilitychange",
    () => {
      if (!document.hidden) void check(true);
    },
    { signal },
  );
  window.addEventListener("focus", () => void check(true), { signal });
  window.addEventListener("scroll", () => settle(), { passive: true, signal });
}

function markNewEntries() {
  if (KIND !== "river") return;
  // The whole feed, not the slice on screen. Paging hides every row outside the current slice,
  // so a boundary read from page two records that page's head as the last thing seen, and coming
  // back to page one finds no boundary at all and marks all of it new.
  const list = $(".rows:not(.search-results)");
  const rows = list ? $$(".row", list) : [];
  if (!rows.length) return;
  const urls = rows.map((row) => row.dataset.url || "").filter(Boolean);
  const key = scopeKey("last-seen-entry");
  const head = storage.read(sessionStorage, key);
  if (head) {
    const boundary = urls.indexOf(head);
    const fresh = new Set(urls.slice(0, boundary === -1 ? urls.length : boundary));
    for (const row of rows) row.classList.toggle("is-new", fresh.has(row.dataset.url || ""));
  }
  // Marking reads the whole list, but only the slice holding the newest entry can say it has been
  // seen. A reader who opened a later slice, or followed a link straight to one, has not looked
  // at what sits above it, and acknowledging those rows would hide them on the way back.
  const pager = $("[data-feed-pager]");
  const newest = !rows[0].hidden && positiveInteger(pager?.dataset.staticPage, 1) === 1;
  if (urls[0] && newest) storage.write(sessionStorage, key, urls[0]);
}

/* ------------------------------------------------------------------ service worker */

function registerWorker() {
  if (AGGR.pwa === false || !("serviceWorker" in navigator)) return;
  navigator.serviceWorker
    .register(new URL("sw.js", BASE).href, { scope: new URL(BASE).pathname, updateViaCache: "none" })
    .catch((error) => console.error("aggr: service worker", error));
}

/* ------------------------------------------------------------------ boot */

/**
 * Everything that belongs to the page on screen, run when it arrives: on load, and again each time
 * another page is swapped in. What it sets up ends with `pageScope`.
 */
function mountPage() {
  safely("dates", () => dates.render());
  safely("selection", () => selection.restore());
  safely("feed-paging", applyFeedPaging);
  safely("new-entries", markNewEntries);
  safely("feed-updates", installFeedUpdates);
  safely("footnotes", installFootnoteTargets);
  safely("heading-anchors", installHeadingAnchors);
  safely("reading-header", installReadingHeader);
  // Every module this page can use is fetched now rather than on the first gesture: waiting for
  // intent puts the download in front of the person who just asked for the thing.
  safely("search", loadSearch);
  safely("preferences-page", () => preparePreferences());
  safely("search-intent", installSearchIntent);
  safely("pictures", settlePictures);
  safely("media", loadMedia);
  safely("speculation", navigation.speculate);
}

function boot() {
  safely("shift-hover", installShiftHover);
  safely("keyboard", installKeyboard);
  safely("shortcut-help", installShortcutHelp);
  safely("preferences", installPreferences);
  safely("keyboard-inset", installKeyboardInset);
  safely("pictures", installPictureStates);
  safely("tap-feedback", installTapFeedback);
  safely("navigation", navigation.install);
  safely("service-worker", registerWorker);
  mountPage();

  // Keep relative times honest while the tab stays open.
  setInterval(() => {
    if (!document.hidden) dates.render();
  }, 60000);
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) dates.render();
  });
  window.addEventListener("pagehide", () => selection.save());

  // Cmd and Ctrl are the same shortcut wearing the platform's own name; only the name differs,
  // so this decides which one the help shows and nothing about how the keys are handled.
  document.documentElement.dataset.platform = /mac|iphone|ipad|ipod/i.test(
    navigator.userAgentData?.platform || navigator.platform || navigator.userAgent,
  )
    ? "apple"
    : "other";

  // One readiness signal, for styles that only apply once enhancement is in place and for the
  // browser contract suite.
  document.documentElement.dataset.aggrReady = "true";
}

// A prerendered page runs before anyone has seen it: writing session state or history there would
// record visits that never happened.
if (document.prerendering) document.addEventListener("prerenderingchange", boot, { once: true });
else boot();

export { dates, selection, $, $$ };
