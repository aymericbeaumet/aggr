import { createSearchSession, mountSearch, type SearchHandle } from "./search";
import type { PreferenceValues, ArticleHeader, BuildManifest, NavigationRequest, NavigationOptions, SwupAdapter, ReaderWindow, ReaderNavigator } from "./contracts";
const readerWindow = window as unknown as ReaderWindow;
const readerNavigator = navigator as ReaderNavigator;
import { createOfflineClient } from "./offline-client";
import { offlineSummary } from "./offline";
import { createDates } from "./dates";
import { createMedia } from "./media";
import { installShiftHover } from "./modifiers";
import { createPreferencesService, mountPreferences } from "./preferences";
import { createScope } from "./lifecycle";
import { createBuildWatcher } from "./build-watcher";
import { createUpdates } from "./updates";
import { mountConnectionStatus } from "./status";
import { mountShortcutHelp } from "./shortcuts";
import { createNavigation, enqueuePrefetch, mountMobileNavigation } from "./navigation";
import { createSelection, selectedLink } from "./selection";
(function () {
  "use strict";
  // Cache the server-rendered page before enhancement adds binding flags or transient UI.
  let initialPage = readerWindow.Swup ? {
    url: location.pathname + location.search,
    html: "<!doctype html>" + document.documentElement.outerHTML
  } : null;
  installShiftHover(document, window);
  function resolveRoot(relative?: string) { return new URL(relative || "./", window.location.href).href; }
  const script = document.querySelector("script[src$='assets/app.js']");
  let BASE = resolveRoot((readerWindow.AGGR && readerWindow.AGGR.base) || (script && (script.getAttribute("src") || "").slice(0, -"assets/app.js".length)) || "./");
  const baseElement = document.querySelector<HTMLBaseElement>("#aggr-base");
  if (baseElement) baseElement.href = BASE;
  let KIND = (readerWindow.AGGR && readerWindow.AGGR.kind) || document.body.getAttribute("data-kind") || "";
  let PWA = readerWindow.AGGR ? readerWindow.AGGR.pwa !== false : true;
  const darkPreference = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)");
  let pageEpoch = 0;
  let swup: SwupAdapter | undefined;

  let restoreListFocus = false;
  const updates = createUpdates(readerWindow.AGGR || {}, navigator.onLine);
  const appScope = createScope();
  const searchSession = createSearchSession(BASE, navigator.onLine);
  appScope.add(() => searchSession.dispose());
  let pageScope = createScope();

  let feedRefresh: Promise<void> | null = null;
  let feedRefreshPending = false;
  let freshNavigation = false;
  let gotoTimer: ReturnType<typeof setTimeout> | undefined;
  let waitingForGoto = false;
  let pendingNewEntries: string[] = [];
  const faviconState: { original: string | null; badged: string | null; loading: boolean; active: boolean } = { original: null, badged: null, loading: false, active: false };
  const preferences = readerWindow.AGGRPreferences;


  const PULL_THRESHOLD = 84;
  const PULL_MAX = 72;
  const PULL_HOLD = 48;
  let pullRefreshState = "idle";
  let pullStartX = 0;
  let pullStartY = 0;
  let pullResetTimer: ReturnType<typeof setTimeout> | undefined;
  let articleHeaderObserver: ResizeObserver | undefined;
  let articleHeaderFrame: number | null = null;
  let articleHeader: ArticleHeader | null = null;
  let articleHeaderNeedsMeasure = true;
  const marginNoteViewport = window.matchMedia("(min-width: 72.0625rem)");

  function $<T extends Element = HTMLElement>(selector: string, root: ParentNode | null = document): T | null { return (root || document).querySelector<T>(selector); }
  function $$<T extends Element = HTMLElement>(selector: string, root: ParentNode | null = document): T[] { return Array.from((root || document).querySelectorAll<T>(selector)); }
  function el<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string | number | boolean | null> = {}, children: Array<Node | null | false> = []): HTMLElementTagNameMap[K] {
    const node = document.createElement(tag);
    Object.keys(attrs || {}).forEach(function (key) {
      if (key === "text") node.textContent = String(attrs[key] ?? "");
      else if (key === "html") node.innerHTML = String(attrs[key] ?? "");
      else node.setAttribute(key, String(attrs[key]));
    });
    (children || []).forEach(function (child) { if (child) node.appendChild(child); });
    return node;
  }
  const dates = createDates({format: function () { return preferences.values["date-format"]; }, afterFormat: applyAgeBands});
  appScope.add(() => dates.dispose());
  function formatTimes(root: ParentNode = document) {
    dates.formatTimes(root);
    searchController?.updateDates();
  }
  const media = createMedia({base: function () { return BASE; }, kind: function () { return KIND; }});
  const enhancePreviewMedia = media.enhancePreview;
  const enhanceArticleMedia = media.enhanceArticle;
  const enhanceVideoPlayer = media.enhanceVideo;

  function enhanceMarginNotes(root: ParentNode) {
    if (!marginNoteViewport.matches) return;
    $$(".body", root).forEach(function (body) {
      if (body.dataset.marginNotesEnhanced === "true") return;
      let count = 0;
      $$(".footnote-ref a[data-footnote-ref]", body).forEach(function (reference) {
        const href = reference.getAttribute("href") || "";
        if (href.charAt(0) !== "#") return;
        let id = href.slice(1);
        try { id = decodeURIComponent(id); } catch (error) { /* keep the literal fragment */ }
        let definition = document.getElementById(id);
        if (!definition || !body.contains(definition)) return;

        const number = reference.textContent.trim();
        const marginId = (reference.id || id + "-reference-" + (count + 1)) + "-note";
        const note = el("aside", {
          "class": "margin-note footnote-margin-note",
          "role": "note",
          "aria-label": "Note " + number,
          "id": marginId
        });
        Array.prototype.slice.call(definition.childNodes).forEach(function (child) {
          note.appendChild(child.cloneNode(true));
        });
        $$(".footnote-backref", note).forEach(function (backref) { backref.remove(); });
        $$('[id]', note).forEach(function (node) { node.removeAttribute("id"); });
        const marker = el("span", { "class": "margin-note-number", "text": number + ". " });
        const firstParagraph = $("p", note);
        if (firstParagraph) firstParagraph.insertBefore(marker, firstParagraph.firstChild);
        else note.insertBefore(marker, note.firstChild);

        reference.removeAttribute("target");
        reference.removeAttribute("rel");
        reference.setAttribute("aria-describedby", note.id);
        reference.parentElement?.insertAdjacentElement("afterend", note);
        reference.addEventListener("click", function (event) {
          if (!marginNoteViewport.matches) return;
          event.preventDefault();
          note.setAttribute("tabindex", "-1");
          note.focus({ preventScroll: true });
        });
        count += 1;
      });
      if (count) body.classList.add("has-margin-notes");
      body.dataset.marginNotesEnhanced = "true";
    });
  }

  function ageBand(iso: string) {
    const age = Math.max(0, Date.now() - Date.parse(iso));
    if (age < 60 * 60 * 1000) return "fresh";
    if (age < 3 * 60 * 60 * 1000) return "h1";
    if (age < 24 * 60 * 60 * 1000) return "h3";
    return "h24";
  }
  function applyAgeBands(root: ParentNode) {
    $$(".rows:not(.search-results)", root).forEach(function (list) {
      $$(".row", list).forEach(function (row) {
        let time = $(".meta time[datetime]", row);
        if (!time) return;
        const band = ageBand(time.getAttribute("datetime") || "");
        if (row.dataset.age === band) return;
        row.classList.remove("age-fresh", "age-h1", "age-h3", "age-h24");
        row.classList.add("age-" + band);
        row.dataset.age = band;
      });
    });
  }
  function externalLinks(root: ParentNode) {
    const installed = isInstalled();
    const scope = new URL(BASE);
    $$<HTMLAnchorElement>('a[href]', root).forEach(function (link) {
      try {
        let url = new URL(link.getAttribute('href') || '', document.baseURI);
        const outOfScope = url.origin !== scope.origin || url.pathname.indexOf(scope.pathname) !== 0;
        if (/^https?:$/.test(url.protocol) && outOfScope) {
          link.setAttribute("data-no-swup", "");
          if (installed) link.removeAttribute("target");
          else link.target = "_blank";
          link.relList.add("noopener", "noreferrer");
          let label = link.dataset.aggrExternalLabel || link.getAttribute("aria-label") || link.textContent.trim();
          const behavior = installed ? "external site" : "opens in a new tab";
          if (label) {
            link.dataset.aggrExternalLabel = label;
            link.setAttribute("aria-label", label + ", " + behavior);
          }
        }
      } catch (error) { /* an incomplete local link */ }
    });
  }

  function entryStateKey(name: string) {
    return "aggr:" + name + ":" + encodeURIComponent(new URL(BASE).pathname);
  }
  function readSessionList(key: string): string[] | null {
    try {
      const value = sessionStorage.getItem(key);
      if (value === null) return null;
      const parsed: unknown = JSON.parse(value);
      return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === "string") : null;
    } catch (error) { return null; }
  }
  function readSessionValue(key: string) {
    try { return sessionStorage.getItem(key); } catch (error) { return null; }
  }
  function writeSessionList(key: string, value: string[]) {
    try { sessionStorage.setItem(key, JSON.stringify(value)); } catch (error) { /* private mode */ }
  }
  function writeSessionValue(key: string, value: string) {
    try { sessionStorage.setItem(key, value); } catch (error) { /* private mode */ }
  }
  function uniqueEntries(entries: string[]) {
    return Array.from(new Set(entries));
  }
  function currentRecentEntries() {
    let entries = readerWindow.AGGR && Array.isArray(readerWindow.AGGR.entries) ? readerWindow.AGGR.entries : [];
    const home = new URL(location.href).pathname === new URL(BASE).pathname;
    if (KIND === "river" && home) {
      entries = entries.concat($$(".rows:not(.search-results) .row[data-url]").map(function (row) {
        return row.dataset.url || "";
      }));
    }
    return uniqueEntries(entries.map(function (entry) {
      try { return new URL(entry, BASE).href; } catch (error) { return null; }
    }).filter((entry): entry is string => entry !== null));
  }
  function detectNewEntries() {
    const seenKey = entryStateKey("last-seen-entry");
    const pendingKey = entryStateKey("new-entries");
    let current = currentRecentEntries();
    const previousHead = readSessionValue(seenKey);
    let pending = readSessionList(pendingKey) || [];
    if (previousHead && current.length) {
      const boundary = current.indexOf(previousHead);
      const additions = current.slice(0, boundary === -1 ? current.length : boundary);
      pending = uniqueEntries(pending.concat(additions));
    }
    if (current.length) writeSessionValue(seenKey, current[0]);
    writeSessionList(pendingKey, pending);
    pendingNewEntries = pending;
  }
  function updateFavicon(active: boolean) {
    let icon = $('link[rel~="icon"]');
    if (!icon) return;
    if (!faviconState.original) faviconState.original = icon.getAttribute("href");
    faviconState.active = active;
    if (!active) {
      icon.setAttribute("href", faviconState.original || "");
      return;
    }
    if (faviconState.badged) {
      icon.setAttribute("href", faviconState.badged);
      return;
    }
    if (faviconState.loading) return;
    faviconState.loading = true;
    const image = new Image();
    image.onload = function () {
      faviconState.loading = false;
      const size = Math.max(image.naturalWidth || 0, 32);
      const canvas = document.createElement("canvas");
      canvas.width = size;
      canvas.height = size;
      let context = canvas.getContext("2d");
      if (!context) return;
      context.drawImage(image, 0, 0, size, size);
      context.beginPath();
      context.arc(size * 0.76, size * 0.24, size * 0.2, 0, Math.PI * 2);
      context.fillStyle = "#e53935";
      context.fill();
      context.lineWidth = Math.max(2, size * 0.06);
      context.strokeStyle = "#ffffff";
      context.stroke();
      faviconState.badged = canvas.toDataURL("image/png");
      if (faviconState.active) icon.setAttribute("href", faviconState.badged);
    };
    image.onerror = function () { faviconState.loading = false; };
    image.src = new URL(faviconState.original || "", document.baseURI).href;
  }
  function acknowledgeNewEntries(entries: string[]) {
    const acknowledged = new Set(entries || []);
    pendingNewEntries = pendingNewEntries.filter(function (entry) { return !acknowledged.has(entry); });
    if (pendingNewEntries.length) writeSessionList(entryStateKey("new-entries"), pendingNewEntries);
    else {
      try { sessionStorage.removeItem(entryStateKey("new-entries")); } catch (error) { /* private mode */ }
    }
    $$(".row.is-new").forEach(function (row) {
      let entry;
      try { entry = new URL(row.dataset.url || "", BASE).href; } catch (error) { return; }
      if (acknowledged.has(entry)) row.classList.remove("is-new");
    });
    updateFavicon(pendingNewEntries.length > 0 && document.visibilityState !== "visible");
  }
  function showNewEntries(root: ParentNode) {
    updateFavicon(pendingNewEntries.length > 0 && document.visibilityState !== "visible");
    if (!pendingNewEntries.length || document.visibilityState !== "visible") return;
    const highlighted: string[] = [];
    $$(".rows:not(.search-results) .row[data-url]", root).forEach(function (row) {
      if (row.hidden) return;
      let entry;
      try { entry = new URL(row.dataset.url || "", BASE).href; } catch (error) { return; }
      if (pendingNewEntries.indexOf(entry) === -1) return;
      row.classList.add("is-new");
      highlighted.push(entry);
    });
    if (highlighted.length) {
      let announcer = $("#aggr-announcer");
      if (announcer) announcer.textContent = highlighted.length + (highlighted.length === 1 ? " new item" : " new items");
      // Flush the starting color before removing it, so the five-second fade starts now.
      const first = $(".row.is-new");
      if (first) first.getBoundingClientRect();
      acknowledgeNewEntries(highlighted);
    }
  }

  const PAGE_HEAD_SELECTOR = [
    'meta[name="description"]',
    'meta[name="robots"]',
    'meta[name="author"]',
    'meta[name^="aggr:"]',
    'meta[property^="og:"]',
    'meta[name^="twitter:"]',
    'meta[property^="article:"]',
    'link[rel="canonical"]',
    'link[rel="first"]',
    'link[rel="last"]',
    'link[rel="prev"]',
    'link[rel="next"]',
    'link[rel="alternate"]',
    'link[rel="search"]',
    'link[rel="service-meta"]',
    'link[rel="type"]',
    'link[rel="via"]',
    'link[rel="original"]',
    'script[type="application/ld+json"]'
  ].join(',');
  function syncPageHead(incoming?: Document | null) {
    if (!incoming || !incoming.head) return;
    $$(PAGE_HEAD_SELECTOR, document.head).forEach(function (node) { node.remove(); });
    $$(PAGE_HEAD_SELECTOR, incoming.head).forEach(function (node) {
      document.head.appendChild(document.importNode(node, true));
    });
  }

  function announceNavigation() {
    let announcer = $("#aggr-announcer");
    if (!announcer) return;
    announcer.textContent = "";
    requestAnimationFrame(function () { announcer.textContent = "Navigated to " + document.title; });
  }

  function singleKeyShortcuts() {
    return preferences.values["single-key-shortcuts"];
  }
  let searchPageSize = preferences.values["feed-page-size"];
  const preferenceService = createPreferencesService(preferences, {
    base: () => BASE, document, window,
    replaceLocation: url => navigation.replaceLocation(url),
    onApply(persist) {
      applyFeedPagination();
      syncOfflinePreferences();
      formatTimes($("#swup") || document);
      waitingForGoto = false;
      clearTimeout(gotoTimer);
      const nextSize = preferences.values["feed-page-size"];
      if (searchController && nextSize !== searchPageSize) searchController.refresh();
      searchPageSize = nextSize;
    }
  });
  appScope.add(() => preferenceService.dispose());
  appScope.add(() => updates.dispose());
  function applyPreferences(state: PreferenceValues, persist: boolean) { preferenceService.apply(state, persist); }

  const FEED_PAGE_PARAMETER = "feed-page";
  function positiveInteger(value: unknown, fallback: number) {
    const parsed = Number.parseInt(String(value), 10);
    return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
  }
  function feedPageUrl(target: string, slice: number) {
    let current = new URL(location.href);
    let url = new URL(target, document.baseURI);
    current.searchParams.forEach(function (value, key) {
      if (key !== FEED_PAGE_PARAMETER && !url.searchParams.has(key)) url.searchParams.set(key, value);
    });
    if (slice > 1) url.searchParams.set(FEED_PAGE_PARAMETER, String(slice));
    else url.searchParams.delete(FEED_PAGE_PARAMETER);
    return url.href;
  }
  function updatePagerLink(pager: HTMLElement, selector: string, target: string, slice: number, visible: boolean) {
    const link = $<HTMLAnchorElement>(selector, pager);
    if (!link) return;
    link.hidden = !visible;
    if (visible) link.href = feedPageUrl(target, slice);
  }
  function applyFeedPagination() {
    if (KIND !== "river" || (searchController && searchController.isActive())) return;
    let list = $(".rows");
    let pager = $("[data-feed-pager]");
    if (!list || !pager) return;

    const rows = $$(".row", list);
    const staticSize = positiveInteger(pager.dataset.staticPageSize, rows.length || 1);
    const staticPage = positiveInteger(pager.dataset.staticPage, 1);
    const staticPages = positiveInteger(pager.dataset.staticPages, 1);
    const totalItems = Math.max(rows.length, positiveInteger(pager.dataset.totalItems, rows.length));
    const preferredSize = positiveInteger(preferences.values["feed-page-size"], 50);
    const pageSize = Math.min(preferredSize, staticSize);
    const slicesPerFullPage = Math.ceil(staticSize / pageSize);
    const slicesOnPage = Math.max(1, Math.ceil(rows.length / pageSize));
    const requestedSlice = positiveInteger(new URL(location.href).searchParams.get(FEED_PAGE_PARAMETER), 1);
    const slice = Math.min(requestedSlice, slicesOnPage);
    const start = (slice - 1) * pageSize;
    const end = Math.min(start + pageSize, rows.length);

    rows.forEach(function (row, index) {
      const hidden = index < start || index >= end;
      if (row.hidden !== hidden) row.hidden = hidden;
      if (hidden && row.classList.contains("is-selected")) row.classList.remove("is-selected");
    });
    list.dataset.feedPage = String(slice);
    list.dataset.feedPageSize = String(pageSize);

    const finalStaticCount = Math.max(0, totalItems - ((staticPages - 1) * staticSize));
    const finalSlices = Math.max(1, Math.ceil(finalStaticCount / pageSize));
    const totalPages = Math.max(1, ((staticPages - 1) * slicesPerFullPage) + finalSlices);
    const currentPage = ((staticPage - 1) * slicesPerFullPage) + slice;
    const hasPrevious = currentPage > 1;
    const hasNext = currentPage < totalPages;
    const previousTarget = slice > 1 ? location.href : pager.dataset.staticPrevious;
    const previousSlice = slice > 1 ? slice - 1 : slicesPerFullPage;
    const nextTarget = slice < slicesOnPage ? location.href : pager.dataset.staticNext;
    const nextSlice = slice < slicesOnPage ? slice + 1 : 1;
    const finalTarget = pager.dataset.staticLast || location.href;

    pager.hidden = totalPages <= 1;
    pager.dataset.page = String(currentPage);
    pager.dataset.pages = String(totalPages);
    const status = $("[data-page-status]", pager);
    if (status) status.textContent = "page " + currentPage + " / " + totalPages;
    updatePagerLink(pager, "[data-page-first]", pager.dataset.staticFirst || location.href, 1, hasPrevious);
    updatePagerLink(pager, "[data-page-previous]", previousTarget || location.href, previousSlice, hasPrevious);
    updatePagerLink(pager, "[data-page-next]", nextTarget || location.href, nextSlice, hasNext);
    updatePagerLink(pager, "[data-page-last]", finalTarget, finalSlices, hasNext);

    const normalized = feedPageUrl(location.href, pageSize < staticSize ? slice : 1);
    navigation.replaceLocation(normalized);
    restoreListCursor(false);
  }
  function listRows() {
    const root = searchController?.isActive() ? $("[data-search-results]") : document;
    if (!root) return [];
    return $$('.rows:not([aria-busy="true"]) .row:not([hidden])', root).filter(row => !row.closest("[hidden]"));
  }
  const rowLink = selectedLink;
  function rowBackgroundLink(target: EventTarget | null) {
    if (!(target instanceof Element) || target.closest('a, button, input, select, textarea, label, summary, [role="button"], [role="link"], [contenteditable]:not([contenteditable="false"])')) return null;
    if (target.closest("[hidden]")) return null;
    const card = target.closest(".article-more-card");
    return card ? $<HTMLAnchorElement>(".article-more-link", card) : rowLink(target.closest(".rows .row:not([hidden])"));
  }
  const selection = createSelection({ rows: listRows, base: () => BASE, pageURL: () => navigation.currentURL(), window,
    renderSelection(url) {
      if (!searchController?.isActive()) return false;
      searchController.select(url);
      return true;
    }
  });
  const selectRow = selection.select;
  const saveListPosition = selection.save;
  const restoreListCursor = selection.restore;
  const moveListCursor = selection.move;
  const navigation = createNavigation({
    location, history, base: () => document.baseURI, swup: () => swup,
    releaseReady: () => updates.snapshot().phase !== "current", beforeNavigate: saveListPosition,
    reselectCurrent: () => {
      const active = document.activeElement;
      if (active instanceof HTMLElement) active.blur();
      window.scrollTo({ top: 0, behavior: 'instant' });
    }
  });
  function openSelectedResult() {
    const row = listRows().find(row => row.classList.contains("is-selected")) || null;
    let link = rowLink(row);
    if (!link) return false;
    selectRow(row, false, false);
    navigate(link.href);
    return true;
  }
  // Article headings are anchors rather than links: a plain click updates the fragment.
  document.addEventListener("click", function (event) {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const origin = event.target instanceof Element ? event.target : null;
    if (!origin || origin.closest("a[href], button, input, select, textarea, summary, [contenteditable]")) return;
    const heading = origin.closest<HTMLElement>(".body :is(h1, h2, h3, h4, h5, h6)[id]");
    if (!heading || !heading.id) return;
    const selected = window.getSelection ? window.getSelection() : null;
    if (selected && !selected.isCollapsed && heading.contains(selected.anchorNode)) return;
    const url = new URL(location.href);
    url.hash = heading.id;
    navigation.replaceLocation(url.href);
    heading.scrollIntoView({ block: "start" });
  });
  document.addEventListener("click", function (event) {
    let target = event.target instanceof Element && event.target.closest("[data-row-open]");
    if (target) selectRow(target.closest(".row"), false, false);
    if (event.target instanceof Element ? event.target.closest<HTMLAnchorElement>("a[href]") : null) saveListPosition();
  }, true);
  (["click", "auxclick"] as const).forEach(function (eventName) {
    document.addEventListener(eventName, function (event) {
      if (event.defaultPrevented || (eventName === "click" ? event.button !== 0 : event.button !== 1)) return;
      let link = rowBackgroundLink(event.target);
      const selection = window.getSelection();
      if (!link || (selection && !selection.isCollapsed)) return;
      event.preventDefault();
      // Synthetic auxclick has no link activation; click preserves middle-button behavior.
      link.dispatchEvent(new MouseEvent("click", {
        bubbles: true, cancelable: true, view: window, button: event.button,
        ctrlKey: event.ctrlKey, metaKey: event.metaKey, shiftKey: event.shiftKey, altKey: event.altKey
      }));
    });
  });
  window.addEventListener("pagehide", saveListPosition);

  let searchController: SearchHandle | null = null;
  let focusSearchAfterNavigation = false;
  function fillSearch() {
    const root = $("[data-search-root]");
    const resultsRoot = $("[data-search-results]");
    if (!root || !resultsRoot) return;
    searchController = mountSearch({
      root: root,
      staticFeed: $("[data-static-feed]"),
      resultsRoot: resultsRoot,
      base: BASE,
      session: searchSession,
      preferences: preferences,
      navigate: navigate,
      getOfflineStatus: function () { return updates.snapshot().offline; },
      onQueryChanged: function () {
        restoreListFocus = false;
        queueMicrotask(updateMenuSelection);
      },
      replaceLocation: function (url) {
        navigation.replaceLocation(url);
      },
      moveSelection: function (direction) { return selection.move(direction, false); },
      openSelected: openSelectedResult,
      onRowsChanged: function () {
        updateMenuSelection();
        externalLinks(resultsRoot);
        formatTimes(resultsRoot);
        const selected = rowLink(resultsRoot.querySelector(".row.is-selected"));
        restoreListCursor(restoreListFocus, restoreListFocus ? undefined : selected?.href);
        restoreListFocus = false;
        if (!searchController?.isActive()) void refreshCurrentFeed();
      }
    });
    const mounted = searchController;
    pageScope.add(() => {
      if (searchController === mounted) searchController = null;
      return mounted.destroy();
    });
    let url = new URL(location.href);
    if (focusSearchAfterNavigation || url.searchParams.has("focus-search")) {
      focusSearchAfterNavigation = false;
      url.searchParams.delete("focus-search");
      navigation.replaceLocation(url.href);
      searchController.focus();
    }
  }
  function resetSearchIndex() {
    searchSession.markStale();
    if (searchController) searchController.refresh();
  }
  function openGlobalSearch() {
    if (searchController) { searchController.focus(); return; }
    focusSearchAfterNavigation = true;
    navigate(BASE);
  }

  const pageRequests = new Map<string, NavigationRequest>();
  let prefetchQueue: string[] = [];
  const prefetchPending = new Map<string, { urgent: boolean }>();
  let prefetchActive = 0;
  function canPrefetch() {
    const connection = readerNavigator.connection;
    return navigator.onLine && document.visibilityState === "visible" && updates.snapshot().phase === "current"
      && !(connection && (connection.saveData || /(^|-)2g$/.test(connection.effectiveType || "")));
  }
  function idle(callback: () => void) {
    if (window.requestIdleCallback) window.requestIdleCallback(callback, { timeout: 250 });
    else setTimeout(callback, 0);
  }
  function drainPrefetch() {
    const client = swup;
    if (!client || client.navigating || !canPrefetch()) return;
    while (prefetchActive < 2 && prefetchQueue.length) {
      // Leave one speculative slot available for hover, focus, and touch intent.
      if (prefetchActive > 0 && !prefetchPending.get(prefetchQueue[0])?.urgent) break;
      const url = prefetchQueue.shift()!;
      if (client.cache.has(url)) { prefetchPending.delete(url); continue; }
      prefetchActive += 1;
      (function (target, token) {
        client.fetchPage(target, { priority: "low" }).catch(function () {
          // A speculative request must never interfere with ordinary navigation.
        }).then(function () {
          prefetchActive -= 1;
          if (prefetchPending.get(target) === token) prefetchPending.delete(target);
          while (client.cache.size > 32) client.cache.delete(client.cache.all.keys().next().value!);
          drainPrefetch();
        });
      })(url, prefetchPending.get(url));
    }
  }
  function prefetchPage(target?: string, urgent?: boolean) {
    if (!swup || !target || !canPrefetch()) return;
    let url;
    try { url = new URL(target, document.baseURI); } catch (error) { return; }
    const scope = new URL(BASE);
    if (url.origin !== scope.origin || url.pathname.indexOf(scope.pathname) !== 0 || url.pathname.slice(-1) !== "/") return;
    url.hash = "";
    url.searchParams.delete("focus-search");
    const key = url.pathname + url.search;
    if (key === location.pathname + location.search || swup.cache.has(key)) return;
    if (prefetchPending.has(key) && !prefetchQueue.includes(key)) return;
    const next = enqueuePrefetch(prefetchQueue, key, !!urgent);
    prefetchQueue.filter(url => !next.includes(url)).forEach(url => prefetchPending.delete(url));
    const pending = prefetchPending.get(key);
    if (pending) pending.urgent ||= !!urgent;
    else prefetchPending.set(key, { urgent: !!urgent });
    prefetchQueue = next;
    drainPrefetch();
  }
  function warmPage() {
    const epoch = pageEpoch;
    idle(function () {
      if (epoch !== pageEpoch || !canPrefetch()) return;
      $$<HTMLAnchorElement>("[data-site-navigation] a[data-route]:not([target])").forEach(function (link) { prefetchPage(link.href); });
      if (KIND === "item") {
        let article = $("article.item");
        if (article) {
          prefetchPage(article.dataset.nextUrl);
          prefetchPage(article.dataset.previousUrl);
          $$<HTMLAnchorElement>(".article-more-card .title", article).forEach(link => prefetchPage(link.href));
        }
      } else {
        listRows().slice(0, 3).forEach(function (row) { let link = rowLink(row); if (link) prefetchPage(link.href); });
      }
      });
  }
  ["pointerover", "pointerdown", "focusin", "touchstart"].forEach(function (eventName) {
    document.addEventListener(eventName, function (event) {
      let link = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>("a[href]") : null;
      if (!link) link = rowBackgroundLink(event.target);
      if (!link || link.target || link.hasAttribute("download") || link.hasAttribute("data-no-swup")) return;
      prefetchPage(link.href, true);
    }, { passive: true });
  });

  function navigate(target: string) { navigation.navigate(target); }
  function firstResultLink() {
    const row = searchController && searchController.isActive()
      ? $("#list .row")
      : $("[data-directory-entry]:not([hidden])");
    return row && ($<HTMLAnchorElement>("a.title", row) || $<HTMLAnchorElement>("a:not([target])", row) || $<HTMLAnchorElement>("a", row));
  }
  function openFirstResult() {
    let link = firstResultLink();
    if (!link) return false;
    navigate(link.href);
    return true;
  }
  function updateMenuSelection() {
    if (appScope.signal.aborted) return;
    const input = $<HTMLInputElement>("#q");
    const hasQuery = searchController?.isActive() ?? !!new URL(location.href).searchParams.get("q")?.trim();
    const searching = KIND === "river" && (hasQuery || !!input && document.activeElement === input);
    $$("[data-site-navigation] [data-kinds]").forEach(function (link) {
      let active = (link.dataset.kinds || "").split(/\s+/).includes(KIND);
      if (link.hasAttribute("data-search-action")) {
        active = searching;
        const target = new URL(BASE);
        const query = hasQuery ? input?.value || new URL(location.href).searchParams.get("q") : "";
        if (query) target.searchParams.set("q", query);
        target.searchParams.set("focus-search", "1");
        const href = target.pathname + target.search;
        if (link.getAttribute("href") !== href) link.setAttribute("href", href);
      } else if (link.hasAttribute("data-feed-action") && searching) active = false;
      if (active) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    });
  }
  for (const name of ["focusin", "focusout"]) {
    document.addEventListener(name, function (event) {
      if (event.target instanceof Element && event.target.id === "q") queueMicrotask(updateMenuSelection);
    }, { signal: appScope.signal });
  }
  function wireMenuNavigation() {
    $$<HTMLAnchorElement>("[data-site-navigation] a[data-route]:not([target])").forEach(function (link) {
      if (link.dataset.navigationBound === "true") return;
      link.dataset.navigationBound = "true";
      link.addEventListener("click", function (event) {
        if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
        event.preventDefault();
        event.stopPropagation();
        if (link.hasAttribute("data-search-action")) openGlobalSearch();
        else navigate(link.href);
      });
    });
  }
  document.addEventListener("click", function (event) {
    if (updates.snapshot().phase === "current" || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    let link = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>("a[href]") : null;
    if (!link || link.hasAttribute("download") || link.target) return;
    let url = new URL(link.href);
    if (url.origin !== location.origin || url.pathname.indexOf(new URL(BASE).pathname) !== 0) return;
    if (url.pathname === location.pathname && url.search === location.search && url.hash) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    navigate(url.href);
  }, true);
  const shortcutDialog = mountShortcutHelp(document, readerWindow.AGGR?.discussions);
  appScope.add(() => shortcutDialog.dispose());
  appScope.add(mountConnectionStatus(document, updates, {
    retry: () => location.reload(),
    refresh: () => {
      if (!updates.beginReload()) return;
      rememberReloadPosition();
      location.reload();
    }
  }));
  function shortcutHelp() { shortcutDialog.toggle(); }
  function beginGoto() {
    waitingForGoto = true;
    clearTimeout(gotoTimer);
    gotoTimer = setTimeout(function () { waitingForGoto = false; }, 1200);
  }
  function finishGoto(key: string) {
    if (!waitingForGoto) return false;
    waitingForGoto = false;
    clearTimeout(gotoTimer);
    if (key === "g") {
      goToBoundary('first');
      return true;
    }
    const routes: Record<string, string> = { f: "", i: "", l: "browse/", p: "preferences/" };
    if (Object.prototype.hasOwnProperty.call(routes, key)) {
      navigate(new URL(routes[key], BASE).href);
      return true;
    }
    if (/^[1-9]$/.test(key)) {
      let entries = (readerWindow.AGGR && readerWindow.AGGR.entries) || [];
      let entry = entries[Number(key) - 1];
      if (entry) {
        navigate(new URL(entry, BASE).href);
        return true;
      }
    }
    return false;
  }
  function isEditing(target: EventTarget | null) {
    return target instanceof Element && !!target.closest('input, textarea, select, button, summary, [role="button"], [role="textbox"], [role="combobox"], [contenteditable]:not([contenteditable="false"])');
  }
  function setStyle(node: HTMLElement, property: string, value: string | number) {
    if (node.style.getPropertyValue(property) !== String(value)) node.style.setProperty(property, String(value));
  }
  function updateArticleHeader() {
    articleHeaderFrame = null;
    const state = articleHeader;
    if (!state) return;
    const y = window.scrollY;
    const folded = y >= 160;
    // Read geometry before changing the folding styles, so scrolling never forces
    // a synchronous layout after an earlier write in this frame.
    let measurements;
    if (articleHeaderNeedsMeasure) {
      const style = getComputedStyle(state.header);
      const top = state.topBar ? Math.max(0, state.topBar.getBoundingClientRect().bottom) : 0;
      measurements = {
        tags: state.labels ? state.labels.getBoundingClientRect().height : null,
        // offsetHeight rounds to whole pixels; the title's transform must not affect this height.
        titleHeight: state.title ? parseFloat(getComputedStyle(state.title).height) : null,
        offset: (style.position === "sticky" ? state.header.getBoundingClientRect().height + (parseFloat(style.top) || 0) : top)
          + (state.fade ? state.fade.getBoundingClientRect().height : 16),
        range: document.documentElement.scrollHeight - window.innerHeight
      };
      state.scrollRange = measurements.range;
      articleHeaderNeedsMeasure = false;
    }
    const progress = Math.max(0, Math.min(1, y / 160));
    setStyle(state.header, "--header-progress", progress);
    if (state.tags && state.folded !== folded) {
      state.tags.inert = folded;
      state.tags.style.visibility = folded ? "hidden" : "visible";
    }
    state.folded = folded;
    if (measurements) {
      if (measurements.tags !== null) setStyle(state.header, "--item-tags-height", measurements.tags + "px");
      if (measurements.titleHeight !== null) setStyle(state.header, "--header-title-height", measurements.titleHeight + "px");
      setStyle(document.documentElement, "--article-header-offset", measurements.offset + "px");
    }
    const readingProgress = state.scrollRange > 0 ? Math.max(0, Math.min(1, y / state.scrollRange)) : 0;
    if (state.progress) setStyle(state.progress, "transform", `scaleX(${readingProgress})`);
  }
  function scheduleArticleHeader() {
    if (articleHeader && !articleHeaderFrame) articleHeaderFrame = requestAnimationFrame(updateArticleHeader);
  }
  function measureArticleHeader() {
    articleHeaderNeedsMeasure = true;
    scheduleArticleHeader();
  }
  function wireArticleHeader() {
    if (articleHeaderObserver) articleHeaderObserver.disconnect();
    document.documentElement.style.removeProperty("--article-header-offset");
    let header = KIND === "item" && $(".itemhead");
    articleHeader = header ? {
      header: header, title: $(".itemhead-title", header), tags: $(".item-tags", header), labels: $(".item-tags-inner", header),
      progress: $(".itemhead-progress", header), fade: $(".itemhead-fade", header), topBar: $(".top"), scrollRange: 0
    } : null;
    articleHeaderNeedsMeasure = true;
    scheduleArticleHeader();
    if (!header) return;
    if (window.ResizeObserver) {
      articleHeaderObserver = new ResizeObserver(measureArticleHeader);
      articleHeaderObserver.observe(header);
      const main = $("main");
      if (main) articleHeaderObserver.observe(main);
      if (articleHeader?.labels) articleHeaderObserver.observe(articleHeader.labels);
      if (articleHeader?.title) articleHeaderObserver.observe(articleHeader.title);
    }
  }
  window.addEventListener("scroll", scheduleArticleHeader, { passive: true });
  window.addEventListener("resize", measureArticleHeader, { passive: true });
  function articleNavigationDirection(key: string) {
    const normalized = key.length === 1 ? key.toLowerCase() : key;
    if (normalized === "k") return "previous";
    if (normalized === "j") return "next";
    return null;
  }
  function pageScroll(event: KeyboardEvent) {
    if (event.altKey || event.metaKey || isEditing(event.target)) return false;
    if (event.ctrlKey && event.shiftKey) return false;
    if (!event.ctrlKey && !singleKeyShortcuts()) return false;
    const key = event.key.toLowerCase();
    let lineScroll = event.ctrlKey && (key === "e" || key === "y");
    if (!lineScroll && key !== "d" && key !== "u") return false;
    const content = $(".body") || $("main") || document.body;
    const style = getComputedStyle(content);
    const line = parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.6;
    let header = $(".itemhead") || $(".top");
    const top = header ? Math.max(0, header.getBoundingClientRect().bottom) : 0;
    const available = Math.max(line, window.innerHeight - top);
    let distance = lineScroll ? line : Math.min(line * preferences.values["scroll-amount"], available * 0.5);
    window.scrollBy({ top: key === "d" || key === "e" ? distance : -distance, behavior: "instant" });
    return true;
  }
  function openArticleExternal(url: string) {
    if (isInstalled()) location.assign(url);
    else window.open(url, "_blank", "noopener,noreferrer");
  }
  function goToBoundary(edge: 'first' | 'last') {
    if (KIND !== 'item') selection.edge(edge);
    window.scrollTo({ top: edge === 'first' ? 0 : document.documentElement.scrollHeight, behavior: 'instant' });
  }
  function articleExternalShortcut(key: string) {
    if (key.length !== 1 || key !== key.toUpperCase()) return false;
    const article = KIND === "item" ? $("article.item") : listRows().find(row => row.classList.contains("is-selected"));
    if (!article) return false;
    if (key === "O") {
      let original = $<HTMLAnchorElement>(".u-bookmark-of", article);
      if (!original) return false;
      openArticleExternal(original.href);
      return true;
    }
    const networks = (readerWindow.AGGR && readerWindow.AGGR.discussions) || [];
    let network = networks.find(function (candidate) { return candidate.shortcut === key; });
    if (!network) return false;
    const found = $$<HTMLAnchorElement>(".discussion[data-discussion]", article).find(function (link) {
      return link.dataset.discussion === network.name;
    });
    const originalUrl = article.dataset.link || "";
    const title = $(".p-name", article);
    let target = found ? found.href : network.url
      .split("{url}").join(encodeURIComponent(originalUrl))
      .split("{title}").join(encodeURIComponent(title ? title.textContent : ""));
    openArticleExternal(target);
    return true;
  }
  document.addEventListener("keydown", function (event) {
    if (event.defaultPrevented || event.isComposing || event.keyCode === 229 || (event.getModifierState && event.getModifierState("AltGraph")) || $("dialog[open]")) return;
    if (!event.altKey && !event.shiftKey && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      openGlobalSearch();
      return;
    }
    if (pageScroll(event)) {
      event.preventDefault();
      return;
    }
    if (event.altKey || event.ctrlKey || event.metaKey || isEditing(event.target)) return;
    if (event.key.length === 1 && !singleKeyShortcuts()) return;
    const focusedLink = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>("a[href]") : null;
    if (event.key === "Enter" && focusedLink) return;
    if (event.key === "?") {
      event.preventDefault();
      shortcutHelp();
      return;
    }
    if (event.key === "G") {
      event.preventDefault();
      waitingForGoto = false;
      clearTimeout(gotoTimer);
      goToBoundary('last');
      return;
    }
    if (!waitingForGoto && articleExternalShortcut(event.key)) {
      event.preventDefault();
      return;
    }
    if (finishGoto(event.key.toLowerCase())) {
      event.preventDefault();
      return;
    }
    if (event.key.toLowerCase() === "g") {
      event.preventDefault();
      beginGoto();
      return;
    }
    if (event.key === "j" || event.key === "k") {
      if (KIND !== "item") {
        if (moveListCursor(event.key === "j" ? 1 : -1)) event.preventDefault();
        return;
      }
    }
    if (event.key === "o" || event.key === "Enter") {
      if (!event.repeat && openSelectedResult()) event.preventDefault();
      return;
    }
    const direction = KIND === "item" && articleNavigationDirection(event.key);
    if (direction) {
      let article = $("article.item");
      let target = article && article.dataset[direction === "next" ? "nextUrl" : "previousUrl"];
      if (!target) return;
      event.preventDefault();
      navigate(target);
      return;
    }
  });

  function isInstalled() {
    return (window.matchMedia && (
      window.matchMedia("(display-mode: standalone)").matches
      || window.matchMedia("(display-mode: minimal-ui)").matches
      || window.matchMedia("(display-mode: fullscreen)").matches
    ))
      || readerNavigator.standalone === true;
  }

  function reloadPositionKey() { return entryStateKey("reload-position"); }
  function rememberReloadPosition() {
    try {
      let active = document.activeElement;
      let dialog = $<HTMLDialogElement>("#shortcut-help");
      sessionStorage.setItem(reloadPositionKey(), JSON.stringify({
        url: location.href,
        x: scrollX,
        y: scrollY,
        focus: active && active.id || "",
        shortcutHelp: !!(dialog && dialog.open)
      }));
    } catch (error) { /* private mode */ }
  }
  function restoreReloadPosition() {
    try {
      let raw = sessionStorage.getItem(reloadPositionKey());
      sessionStorage.removeItem(reloadPositionKey());
      if (!raw) return;
      const position = JSON.parse(raw);
      if (position.url !== location.href) return;
      requestAnimationFrame(function () {
        requestAnimationFrame(function () {
          let dialog = $<HTMLDialogElement>("#shortcut-help");
          if (position.shortcutHelp && dialog && !dialog.open) {
            if (dialog.showModal) dialog.showModal();
            else dialog.setAttribute("open", "");
            const title = $("#shortcut-help-title", dialog);
            if (title) title.focus({ preventScroll: true });
          }
          const focus = position.focus && document.getElementById(position.focus);
          if (focus?.id === "q" && searchController) searchController.focus({ restore: true });
          else if (focus && focus.focus) focus.focus({ preventScroll: true });
          window.scrollTo(position.x || 0, position.y || 0);
        });
      });
    } catch (error) { /* malformed or unavailable session state */ }
  }

  const showConnectionStatus = updates.notice;
  function updateConnectionStatus() { searchSession.setOnline(navigator.onLine); updates.setOnline(navigator.onLine); }

  function setPullRefreshState(next: string, distance: number) {
    let root = document.documentElement;
    const indicator = $("#pull-refresh");
    let label = $("#pull-refresh-label");
    let changed = pullRefreshState !== next;
    pullRefreshState = next;
    if (next === "idle") {
      delete root.dataset.pullState;
      root.style.removeProperty("--pull-distance");
      if (indicator) indicator.setAttribute("aria-hidden", "true");
      return;
    }
    root.dataset.pullState = next;
    root.style.setProperty("--pull-distance", Math.max(0, distance || 0) + "px");
    if (indicator) indicator.setAttribute("aria-hidden", "false");
    if (!changed || !label) return;
    if (next === "armed") label.textContent = "Release to refresh";
    else if (next === "refreshing") label.textContent = "Refreshing…";
    else label.textContent = "Pull to refresh";
  }

  function settlePullRefresh() {
    if (pullRefreshState === "idle" || pullRefreshState === "refreshing") return;
    clearTimeout(pullResetTimer);
    setPullRefreshState("settling", 0);
    pullResetTimer = setTimeout(function () {
      setPullRefreshState("idle", 0);
    }, 190);
  }

  function wireTouchPullRefresh() {
    let root = document.documentElement;
    if (root.dataset.pullRefreshBound === "true") return;
    if (!PWA || !isInstalled() || !(navigator.maxTouchPoints > 0 || "ontouchstart" in window)) return;
    root.dataset.pullRefreshBound = "true";

    document.addEventListener("touchstart", function (event) {
      if (pullRefreshState === "refreshing" || event.touches.length !== 1) return;
      if (!isInstalled() || window.scrollY > 0 || $("dialog[open]") || isEditing(event.target)) return;
      clearTimeout(pullResetTimer);
      pullStartX = event.touches[0].clientX;
      pullStartY = event.touches[0].clientY;
      setPullRefreshState("tracking", 0);
    }, { passive: true });

    document.addEventListener("touchmove", function (event) {
      if (["tracking", "pulling", "armed"].indexOf(pullRefreshState) === -1) return;
      if (event.touches.length !== 1 || window.scrollY > 0) {
        settlePullRefresh();
        return;
      }
      const deltaX = Math.abs(event.touches[0].clientX - pullStartX);
      const deltaY = event.touches[0].clientY - pullStartY;
      if (deltaY <= 0 || deltaX > deltaY) {
        settlePullRefresh();
        return;
      }
      if (deltaY < 6) return;
      if (event.cancelable) event.preventDefault();
      let distance = Math.min(PULL_MAX, Math.round(deltaY * 0.55));
      if (deltaY >= PULL_THRESHOLD) setPullRefreshState("armed", distance);
      else setPullRefreshState("pulling", distance);
    }, { passive: false });

    document.addEventListener("touchend", function () {
      if (pullRefreshState === "armed") {
        clearTimeout(pullResetTimer);
        setPullRefreshState("refreshing", PULL_HOLD);
        showConnectionStatus("Refreshing for new items…", false, false);
        rememberReloadPosition();
        location.reload();
      } else {
        settlePullRefresh();
      }
    }, { passive: true });
    document.addEventListener("touchcancel", settlePullRefresh, { passive: true });
  }

  function clearNavigationCache() {
    prefetchQueue = [];
    prefetchPending.clear();
    if (pageRequests) pageRequests.forEach(function (request) {
      if (request.speculative) request.controller?.abort();
    });
    if (swup && swup.cache) swup.cache.clear();
  }

  function refreshCurrentFeed(): Promise<void> {
    if (searchController && searchController.isActive()) {
      return Promise.resolve();
    }
    if (["river", "source", "category", "tag"].indexOf(KIND) === -1) {
      feedRefreshPending = false;
      return Promise.resolve();
    }
    const displayed = $("#aggr-page");
    if (displayed && displayed.dataset.contentVersion !== updates.snapshot().contentVersion) feedRefreshPending = true;
    if (!feedRefreshPending || document.visibilityState !== "visible") return Promise.resolve();
    if (feedRefresh) return feedRefresh;
    if (swup && swup.navigating) return Promise.resolve();
    let url = location.href;
    const epoch = pageEpoch;
    let version = updates.snapshot().contentVersion;
    const cancellation = new AbortController();
    const timeout = setTimeout(function () { cancellation.abort(); }, 10000);
    feedRefresh = fetch(url, {
      cache: "no-store", signal: cancellation.signal,
      headers: {"Accept": "text/html", "X-Requested-With": "swup"}
    }).then(function (response) {
      if (!response.ok) throw new Error("Feed unavailable");
      return response.text();
    }).then(async function (html) {
      if (location.href !== url || epoch !== pageEpoch || version !== updates.snapshot().contentVersion) return;
      let incoming = new DOMParser().parseFromString(html, "text/html");
      let page = $("#aggr-page", incoming);
      let next = $("#swup", incoming);
      let current = $("#swup");
      if (!page || !next || !current || page.dataset.kind !== KIND) return;
      if (page.dataset.appVersion !== updates.snapshot().availableAppVersion) return;
      // During a non-atomic deployment, retry instead of replacing a newer list with old HTML.
      if (page.dataset.contentVersion !== version) return;
      let active = document.activeElement;
      const focusedHref = active instanceof HTMLAnchorElement && current.contains(active) ? active.href : null;
      const selected = rowLink(listRows().find(row => row.classList.contains("is-selected")));
      const selectedUrl = selected && selected.href;
      const x = scrollX, y = scrollY;
      const top = ($(".top")?.getBoundingClientRect().bottom || 0);
      const anchor = y > 0 && listRows().find(function (row) { return row.getBoundingClientRect().bottom > top; });
      const anchorUrl = anchor ? rowLink(anchor)?.href : undefined;
      const anchorTop = anchor ? anchor.getBoundingClientRect().top : 0;
      let input = $<HTMLInputElement>("#q");
      const selection: [number | null, number | null, "forward" | "backward" | "none" | null] | null = input && active === input ? [input.selectionStart, input.selectionEnd, input.selectionDirection] : null;
      saveListPosition();
      if (swup) swup.cache.set(url, {url: url, html: html});
      const replacedScope = pageScope;
      await replacedScope.dispose();
      if (replacedScope !== pageScope || current !== $("#swup") || location.href !== url || epoch !== pageEpoch || version !== updates.snapshot().contentVersion) return;
      current.replaceChildren.apply(current, Array.from(next.childNodes).map(function (node) { return document.importNode(node, true); }));
      document.title = incoming.title;
      syncPageHead(incoming);
      pageEpoch += 1;
      feedRefreshPending = false;
      bootPage();
      input = $<HTMLInputElement>("#q");
      if (input && selection && selection[0] !== null) {
        searchController?.focus({ restore: true });
        input.setSelectionRange(selection[0], selection[1], selection[2] || undefined);
      }
      restoreListCursor(false, selectedUrl);
      if (focusedHref) {
        const focus = $$<HTMLAnchorElement>("a[href]", current).find(function (link) { return link.href === focusedHref && link.getClientRects().length; });
        if (focus) focus.focus({preventScroll: true});
      }
      let retained = anchorUrl && listRows().find(function (row) { return rowLink(row)?.href === anchorUrl; });
      window.scrollTo(x, retained ? scrollY + retained.getBoundingClientRect().top - anchorTop : y);
      warmPage();
    }).catch(function () { /* retain the current feed and retry after reconnecting */ }).finally(function () {
      clearTimeout(timeout);
      feedRefresh = null;
    });
    return feedRefresh;
  }

  function applyBuildUpdate(build: BuildManifest | null) {
    if (!build || typeof build.app_version !== "string" || !build.app_version || typeof build.content_version !== "string" || !build.content_version) return Promise.resolve();
    const transition = updates.receive(build);
    if (transition.appChanged) {
      document.documentElement.dataset.updateState = "ready";
      swup?.cache.clear();
    }
    if (transition.contentChanged) {
      freshNavigation = true;
      clearNavigationCache();
      pageEpoch += 1;
      resetSearchIndex();
      feedRefreshPending = true;
      if (Array.isArray(build.entries)) {
        readerWindow.AGGR.entries = build.entries.filter(function (entry) {
          if (typeof entry !== "string") return false;
          try { return new URL(entry, BASE).href.startsWith(BASE); } catch (error) { return false; }
        });
        detectNewEntries();
      }
      showNewEntries($("#swup") || document);
    }
    return refreshCurrentFeed();
  }

  const buildWatcher = createBuildWatcher({
    window, document, base: () => BASE, enabled: () => !!updates.snapshot().appVersion,
    fetch: (...args) => window.fetch(...args), apply: applyBuildUpdate
  });
  const checkForUpdates = buildWatcher.check;
  appScope.add(() => buildWatcher.dispose());


  function renderOfflineStatus() {
    let state = updates.snapshot().offline;
    preferenceService.setOfflineSummary(offlineSummary(state, PWA));
    let list = $("#offline-articles");
    if (list && state) {
      list.replaceChildren();
      state.saved.forEach(function (item) {
        const row = el("li");
        row.appendChild(el("a", {href: new URL(item.url, BASE).href, text: item.title}));
        list.appendChild(row);
      });
      const empty = $("#offline-empty");
      if (empty) empty.hidden = state.saved.length > 0;
    }
  }
  const offlineClient = createOfflineClient({
    base: () => BASE, pwa: () => PWA, kind: () => KIND, count: () => preferences.values["offline-items"],
    onBuild(build) {
      // A previous worker can control a newer document; confirm versions with the manifest.
      if (build.app_version !== updates.snapshot().availableAppVersion || build.content_version !== updates.snapshot().contentVersion) void checkForUpdates(true);
      if (build.content_version === updates.snapshot().contentVersion) freshNavigation = false;
    },
    onStatus(state) {
      updates.setOffline(state);
      renderOfflineStatus();
      searchController?.updateOfflineStatus();
    },
    onControllerChange() {
      clearNavigationCache();
      void checkForUpdates().then(() => {
        if (searchController && updates.snapshot().phase === "current") {
          pageEpoch += 1;
          resetSearchIndex();
        }
      });
    }
  });
  appScope.add(() => offlineClient.dispose());
  function syncOfflinePreferences(force?: boolean) {
    renderOfflineStatus();
    offlineClient.configure(force);
  }

  function bootPage() {
    pageScope = createScope();
    pageScope.add(() => media.dispose());
    pageScope.add(() => {
      articleHeaderObserver?.disconnect();
      if (articleHeaderFrame !== null) cancelAnimationFrame(articleHeaderFrame);
      articleHeaderObserver = undefined;
      articleHeaderFrame = null;
      articleHeader = null;
    });
    navigation.acceptPage();
    let page = $("#aggr-page");
    if (page) {
      BASE = resolveRoot(page.dataset.root);
      KIND = page.dataset.kind || "";
      if ($("#aggr-base")?.getAttribute("href") !== BASE) $("#aggr-base")?.setAttribute("href", BASE);
      if (document.body.dataset.kind !== KIND) document.body.dataset.kind = KIND;
      $$("[data-site-navigation] [data-route]").forEach(function (link) {
        const target = new URL(link.dataset.route || "", document.baseURI);
        const href = target.pathname + target.search + target.hash;
        if (link.getAttribute("href") !== href) link.setAttribute("href", href);
      });
      updateMenuSelection();
    }
    wireMenuNavigation();

    wireTouchPullRefresh();
    preferenceService.importLocation(KIND);
    const preferencePanel = mountPreferences($("#swup") || document, preferenceService, { onShortcuts: shortcutHelp });
    pageScope.add(() => preferencePanel.dispose());
    applyPreferences(preferences.values, false);
    enhanceMarginNotes($("#swup") || document);
    enhancePreviewMedia($("#swup") || document);
    enhanceVideoPlayer();
    enhanceArticleMedia($("#swup") || document);
    wireArticleHeader();
    externalLinks(pageEpoch ? $("#swup") || document : document);
    fillSearch();
    if (!searchController?.isActive()) restoreListCursor(restoreListFocus && !swup);
    showNewEntries($("#swup") || document);
  }

  window.addEventListener("appinstalled", function () {
    externalLinks(document);
    wireTouchPullRefresh();
  });
  window.addEventListener("offline", updateConnectionStatus);
  window.addEventListener("online", updateConnectionStatus);

  if (window.performance && performance.getEntriesByType) {
    const navigationEntry = performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined;
    restoreListFocus = !!(navigationEntry && navigationEntry.type === "back_forward");
  }
  detectNewEntries();
  bootPage();
  restoreReloadPosition();
  window.addEventListener("pagehide", event => {
    if (event.persisted) return;
    void pageScope.dispose().catch(() => {});
    void appScope.dispose().catch(() => {});
  });
  const mobileBar = $(".mobile-tabs");
  if (mobileBar) appScope.add(mountMobileNavigation(mobileBar, window));
  const topBar = $(".top");
  if (topBar && window.ResizeObserver) {
    const syncTopNavOffset = function () {
      setStyle(document.documentElement, "--top-nav-offset", topBar.getBoundingClientRect().height + "px");
    };
    new ResizeObserver(syncTopNavOffset).observe(topBar);
    syncTopNavOffset();
  }
  document.addEventListener("visibilitychange", function () {
    if (document.visibilityState === "visible") {
      formatTimes($("#swup") || document);
      showNewEntries($("#swup") || document);
    }
  });
  if (readerWindow.Swup) {
    readerWindow.swup = swup = new readerWindow.Swup({ containers: ["#swup"], cache: true, animationSelector: false, native: false, animateHistoryBrowsing: false });
    
    const fetchPage = swup.fetchPage.bind(swup);
    swup.hooks.before("fetch:request", function (visit, request) {
      let options = request.options;
      let speculative = options.prefetchSignal;
      if (!speculative) return;
      delete options.prefetchSignal;
      // Swup supplies its own timeout signal; retain both cancellation paths.
      if (AbortSignal.any) options.signal = AbortSignal.any([options.signal, speculative]);
      else {
        let controller = new AbortController();
        [options.signal, speculative].forEach(function (signal) {
          if (signal.aborted) controller.abort();
          else signal.addEventListener("abort", function () { controller.abort(); }, { once: true });
        });
        options.signal = controller.signal;
      }
    });
    swup.fetchPage = function (target, options) {
      options = options || {};
      if (options.method && options.method !== "GET") return fetchPage(target, options);
      let url = new URL(target, document.baseURI);
      const key = url.pathname + url.search;
      const existing = pageRequests.get(key);
      if (existing) {
        if (options.priority !== "low") existing.speculative = false;
        return existing.promise;
      }
      const speculative = options.priority === "low";
      const controller = speculative ? new AbortController() : undefined;
      const requestOptions: NavigationOptions = { priority: "high", ...options };
      if (freshNavigation) requestOptions.cache = "no-store";
      if (controller) requestOptions.prefetchSignal = controller.signal;
      const record: NavigationRequest = {
        speculative, controller,
        promise: fetchPage(target, requestOptions).then(function (page) {
          if (pageRequests.get(key) === record) pageRequests.delete(key);
          return page;
        }, function (error: unknown) {
          if (pageRequests.get(key) === record) pageRequests.delete(key);
          throw error;
        })
      };
      pageRequests.set(key, record);
      return record.promise;
    };
    if (initialPage) {
      swup.cache.set(initialPage.url, initialPage);
      const initialUrl = location.pathname + location.search;
      if (initialUrl !== initialPage.url) swup.cache.set(initialUrl, { url: initialUrl, html: initialPage.html });
      initialPage = null;
    }
    swup.hooks.on("visit:start", function (visit) {
      visit.animation.animate = false;
      saveListPosition();
      pageEpoch += 1;
      prefetchQueue.forEach(function (url) { prefetchPending.delete(url); });
      prefetchQueue = [];
      const destination = new URL(visit.to.url, document.baseURI);
      const destinationKey = destination.pathname + destination.search;
      pageRequests.forEach(function (request, key) {
        if (!request.speculative) return;
        if (key === destinationKey) request.speculative = false;
        else {
          pageRequests.delete(key);
          prefetchPending.delete(key);
          request.controller?.abort();
        }
      });
      restoreListFocus = !!(visit && visit.history && visit.history.popstate);
    });
    swup.hooks.on("visit:end", warmPage);
    swup.hooks.before("content:replace", async function (visit) {
      await pageScope.dispose();
      syncPageHead(visit && visit.to && visit.to.document);
    });
    swup.hooks.on("page:view", function () {
      bootPage();
      refreshCurrentFeed();
      announceNavigation();
      const searching = !!searchController?.isActive();
      const restored = restoreListFocus && !searching && restoreListCursor(true);
      let target = restored || (searching && restoreListFocus) || document.activeElement === $<HTMLInputElement>("#q") ? null : searching || KIND === "search" ? $<HTMLInputElement>("#q") : $("#swup");
      // Canonical search URLs use the river page; its asynchronous rows restore the cursor.
      if (!searching) restoreListFocus = false;
      if (target) {
        if (target.id === "swup") {
          target.dataset.navigationFocus = "";
          target.addEventListener("blur", function () {
            target.removeAttribute("data-navigation-focus");
          }, { once: true });
        }
        target.focus({ preventScroll: true });
      }
    });
  }
  warmPage();

  if (darkPreference) {
    const syncAutoTheme = function () {
      if (document.documentElement.dataset.theme === "auto") {
        preferenceService.refreshTheme();
      }
    };
    if (darkPreference.addEventListener) darkPreference.addEventListener("change", syncAutoTheme);
    else if (darkPreference.addListener) darkPreference.addListener(syncAutoTheme);
  }
  const syncMarginNotes = function () {
    if (!marginNoteViewport.matches) return;
    enhanceMarginNotes($("#swup") || document);
    externalLinks($(".body") || document);
  };
  if (marginNoteViewport.addEventListener) marginNoteViewport.addEventListener("change", syncMarginNotes);
  else if (marginNoteViewport.addListener) marginNoteViewport.addListener(syncMarginNotes);
  offlineClient.start();
  buildWatcher.start();
  setInterval(function () {
    if (document.visibilityState === "visible") formatTimes($("#swup") || document);
  }, 60 * 1000);
})();
