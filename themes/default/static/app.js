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
      store.setItem(key, value);
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
      if (selected) write({ url: selected.href });
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
        location.assign(new URL(url || "", BASE).href);
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
    location.assign(new URL("?focus-search=1", BASE).href);
    return;
  }
  loadSearch();
  // Focus first without moving, then decide: the field's own box is what we measure.
  field.focus({ preventScroll: true });
  revealSearchField(field);
  field.select();
}

let searchModule;
/** The search engine is a separate file, fetched with the page that can use it. */
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
}

/* ------------------------------------------------------------------ pictures */

/**
 * Every picture is painted over the placeholder the build inlined behind it, and falls back to its
 * alt text when it never arrives. Two capturing listeners cover the whole document, including the
 * rows search adds later, so no page has to load a module to show a picture honestly.
 */
function installPictureStates() {
  const frame = (image) =>
    image.closest(".article-picture, .article-lead, .preview-media, .audio-cover, .media-frame");
  // A picture that arrives after a failure, or fails after arriving, must not keep both marks:
  // the same source is retried whenever a reader comes back to a page that had no network.
  const mark = (box, loaded) => {
    box.classList.toggle("is-loaded", loaded);
    box.classList.toggle("is-error", !loaded);
  };
  const settle = (event, loaded) => {
    const target = event.target;
    if (!(target instanceof HTMLImageElement)) return;
    const box = frame(target);
    if (box) mark(box, loaded);
  };
  document.addEventListener("load", (event) => settle(event, true), true);
  document.addEventListener("error", (event) => settle(event, false), true);
  // A picture the browser had already finished with never fires either event here.
  for (const image of $$("img")) {
    if (!(image instanceof HTMLImageElement) || !image.complete) continue;
    const box = frame(image);
    if (box) mark(box, image.naturalWidth > 0);
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
  import(new URL(AGGR.assets.media, document.baseURI).href)
    .then((module) => module.mount())
    .catch((error) => console.error("aggr: media", error));
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
    const id = decodeURIComponent(location.hash.slice(1));
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
  window.addEventListener("hashchange", paired);
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
    if (record && location.hash.slice(1) !== id) {
      history.pushState(history.state, "", "#" + encodeURIComponent(id));
    }
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
    const id = decodeURIComponent(location.hash.slice(1));
    if (id && document.getElementById(id)?.closest(".body")) jump(id, false);
  };
  window.addEventListener("hashchange", settle);
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
  // The controls ship disabled so they cannot take a value nothing would save.
  const controls = /** @type {HTMLFieldSetElement | null} */ ($("#preferences-controls"));
  if (controls) controls.disabled = false;
  syncControls();
  refreshThemeColor();
  importFragment();
}

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
      const response = await fetch(location.href, { cache: "no-store" });
      if (!response.ok) return;
      const page = new DOMParser().parseFromString(await response.text(), "text/html");
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
  setInterval(() => {
    if (!document.hidden) void check();
  }, UPDATE_INTERVAL);
  // Opening the app again is the moment a reader expects to be caught up.
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) void check(true);
  });
  window.addEventListener("focus", () => void check(true));
  window.addEventListener("scroll", () => settle(), { passive: true });
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

function boot() {
  safely("shift-hover", installShiftHover);
  safely("dates", () => dates.render());
  safely("selection", () => selection.restore());
  safely("feed-paging", applyFeedPaging);
  safely("new-entries", markNewEntries);
  safely("feed-updates", installFeedUpdates);
  safely("keyboard", installKeyboard);
  safely("shortcut-help", installShortcutHelp);
  safely("footnotes", installFootnoteTargets);
  safely("heading-anchors", installHeadingAnchors);
  // Every module this page can use is fetched now rather than on the first gesture: waiting for
  // intent puts the download in front of the person who just asked for the thing.
  safely("search", loadSearch);
  safely("preferences", installPreferences);
  safely("search-intent", installSearchIntent);
  safely("keyboard-inset", installKeyboardInset);
  safely("pictures", installPictureStates);
  safely("tap-feedback", installTapFeedback);
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
