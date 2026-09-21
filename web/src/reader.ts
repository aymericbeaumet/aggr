import { mountArticleSwipe } from "./reader/article-swipe";
import { createSearchSession, inertSearchHandle, mountSearch, type SearchHandle } from "./search";
import type { PreferenceValues, BuildManifest, NavigationRequest, NavigationOptions, SwupAdapter, ReaderWindow, ReaderNavigator } from "./contracts";
const readerWindow = window as unknown as ReaderWindow;
const readerNavigator = navigator as ReaderNavigator;
import { createOfflineClient } from "./offline-client";
import { offlineSummary } from "./offline";
import { createDates } from "./dates";
import { createMedia } from "./media";
import { installShiftHover } from "./modifiers";
import { createPreferencesService, mountPreferences } from "./preferences";
import { createScope, safely } from "./lifecycle";
import { createBuildWatcher } from "./build-watcher";
import { createUpdates } from "./updates";
import { mountConnectionStatus } from "./status";
import { inertShortcutHelp, mountShortcutHelp } from "./shortcuts";
import { createNavigation, enqueuePrefetch, mountMobileNavigation, prefetchKey } from "./navigation";
import { createSelection, selectedLink } from "./selection";
import { mountSelectionSharing } from "./share-selection";
import { announce } from "./announce";
import { originalLabel } from "./labels";
import { $, $$, el, setStyle } from "./reader/dom";
import { syncPageHead } from "./reader/page-head";
import { enhanceMarginNotes } from "./reader/margin-notes";
import { createArticleHeader } from "./reader/article-header";
import { ageBand, entryStateKey, mergeNewEntries, remainingEntries, resolveEntries, scopedEntries } from "./reader/entries";
import { FEED_PAGE_PARAMETER, feedPageUrl, paginationLayout, type PagerLink } from "./reader/pagination";
import { pullLabel, wireTouchPullRefresh, type PullPhase } from "./reader/pull-refresh";
import { articleNavigationDirection, articleNavigationTarget, externalShortcutKey, externalShortcutTarget, gotoRoute, lineHeight, scrollDistance, scrollShortcut } from "./reader/shortcuts";
(function () {
  "use strict";
  // Cache the server-rendered page before enhancement adds binding flags or transient UI.
  let initialPage = readerWindow.Swup ? {
    url: location.pathname + location.search,
    html: "<!doctype html>" + document.documentElement.outerHTML
  } : null;
  safely("shift-hover", () => installShiftHover(document, window));
  function resolveRoot(relative?: string) { return new URL(relative || "./", window.location.href).href; }
  const script = document.querySelector("script[src$='assets/app.js']");
  let BASE = resolveRoot((readerWindow.AGGR && readerWindow.AGGR.base) || (script && (script.getAttribute("src") || "").slice(0, -"assets/app.js".length)) || "./");
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


  const marginNoteViewport = window.matchMedia("(min-width: 72.0625rem)");
  const articleHeader = createArticleHeader({ document, window, kind: () => KIND });

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
          let label = originalLabel(link) || link.textContent.trim();
          const behavior = installed ? "external site" : "opens in a new tab";
          if (label) {
            link.dataset.aggrExternalLabel = label;
            link.setAttribute("aria-label", label + ", " + behavior);
          }
        }
      } catch (error) { /* an incomplete local link */ }
    });
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
  function currentRecentEntries() {
    let entries = readerWindow.AGGR && Array.isArray(readerWindow.AGGR.entries) ? readerWindow.AGGR.entries : [];
    const home = new URL(location.href).pathname === new URL(BASE).pathname;
    if (KIND === "river" && home) {
      entries = entries.concat($$(".rows:not(.search-results) .row[data-url]").map(function (row) {
        return row.dataset.url || "";
      }));
    }
    return resolveEntries(entries, BASE);
  }
  function detectNewEntries() {
    const seenKey = entryStateKey("last-seen-entry", BASE);
    const pendingKey = entryStateKey("new-entries", BASE);
    let current = currentRecentEntries();
    const pending = mergeNewEntries(readSessionValue(seenKey), current, readSessionList(pendingKey) || []);
    if (current.length) writeSessionValue(seenKey, current[0]);
    writeSessionList(pendingKey, pending);
    pendingNewEntries = pending;
  }
  function updateFavicon(active: boolean) {
    let icon = $('link[rel~="icon"]');
    if (!icon) return;
    // Resolve now: the attribute is relative to the page that rendered it, and later pages sit at other depths.
    if (!faviconState.original) faviconState.original = new URL(icon.getAttribute("href") || "", location.href).href;
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
    image.src = faviconState.original || "";
  }
  function acknowledgeNewEntries(entries: string[]) {
    const acknowledged = new Set(entries || []);
    pendingNewEntries = remainingEntries(pendingNewEntries, acknowledged);
    if (pendingNewEntries.length) writeSessionList(entryStateKey("new-entries", BASE), pendingNewEntries);
    else {
      try { sessionStorage.removeItem(entryStateKey("new-entries", BASE)); } catch (error) { /* private mode */ }
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
      announce(highlighted.length + (highlighted.length === 1 ? " new item" : " new items"));
      // Flush the starting color before removing it, so the five-second fade starts now.
      const first = $(".row.is-new");
      if (first) first.getBoundingClientRect();
      acknowledgeNewEntries(highlighted);
    }
  }

  function announceNavigation() {
    announce("Navigated to " + document.title);
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

  function updatePagerLink(pager: HTMLElement, selector: string, link: PagerLink) {
    const anchor = $<HTMLAnchorElement>(selector, pager);
    if (!anchor) return;
    anchor.hidden = !link.visible;
    if (link.visible) anchor.href = feedPageUrl(link.target, link.slice, location.href, BASE);
  }
  function applyFeedPagination() {
    if (KIND !== "river" || (searchController && searchController.isActive())) return;
    let list = $(".rows");
    let pager = $("[data-feed-pager]");
    if (!list || !pager) return;

    const rows = $$(".row", list);
    const layout = paginationLayout(rows.length, pager.dataset, preferences.values["feed-page-size"], new URL(location.href).searchParams.get(FEED_PAGE_PARAMETER), location.href);
    rows.forEach(function (row, index) {
      const hidden = index < layout.start || index >= layout.end;
      if (row.hidden !== hidden) row.hidden = hidden;
      if (hidden && row.classList.contains("is-selected")) row.classList.remove("is-selected");
    });
    list.dataset.feedPage = String(layout.slice);
    list.dataset.feedPageSize = String(layout.pageSize);

    pager.hidden = layout.totalPages <= 1;
    pager.dataset.page = String(layout.currentPage);
    pager.dataset.pages = String(layout.totalPages);
    const status = $("[data-page-status]", pager);
    if (status) status.textContent = "page " + layout.currentPage + " / " + layout.totalPages;
    updatePagerLink(pager, "[data-page-first]", layout.first);
    updatePagerLink(pager, "[data-page-previous]", layout.previous);
    updatePagerLink(pager, "[data-page-next]", layout.next);
    updatePagerLink(pager, "[data-page-last]", layout.last);

    navigation.replaceLocation(feedPageUrl(location.href, layout.normalizedSlice, location.href, BASE));
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
    // Site-relative targets resolve against the site root; absolute ones are unaffected.
    location, history, base: () => BASE, swup: () => swup,
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
    // A failed mount leaves the static form in place and an inert handle behind so the rest of the page boots.
    searchController = safely("search", () => mountSearch({
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
    })) ?? inertSearchHandle();
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
    const key = prefetchKey(target, document.baseURI, BASE, location);
    if (!key || swup.cache.has(key)) return;
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
          prefetchPage(articleNavigationTarget("next", article.dataset, BASE));
          prefetchPage(articleNavigationTarget("previous", article.dataset, BASE));
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
    $$("[data-site-navigation] [data-kinds]").forEach(function (link) {
      const active = (link.dataset.kinds || "").split(/\s+/).includes(KIND);
      if (active) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    });
  }
  function wireMenuNavigation() {
    $$<HTMLAnchorElement>("[data-site-navigation] a[data-route]:not([target])").forEach(function (link) {
      if (link.dataset.navigationBound === "true") return;
      link.dataset.navigationBound = "true";
      link.addEventListener("click", function (event) {
        if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
        event.preventDefault();
        event.stopPropagation();
        navigate(link.href);
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
  const shortcutDialog = safely("shortcut-help", () => mountShortcutHelp(document, readerWindow.AGGR?.discussions)) ?? inertShortcutHelp();
  appScope.add(() => shortcutDialog.dispose());
  safely("connection-status", () => appScope.add(mountConnectionStatus(document, updates, {
    retry: () => location.reload(),
    refresh: () => {
      if (!updates.beginReload()) return;
      rememberReloadPosition();
      location.reload();
    }
  })));
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
    const target = gotoRoute(key, (readerWindow.AGGR && readerWindow.AGGR.entries) || [], BASE);
    if (!target) return false;
    if (target.type === "first") goToBoundary('first');
    else navigate(target.url);
    return true;
  }
  function isEditing(target: EventTarget | null) {
    return target instanceof Element && !!target.closest('input, textarea, select, button, summary, [role="button"], [role="textbox"], [role="combobox"], [contenteditable]:not([contenteditable="false"])');
  }
  function pageScroll(event: KeyboardEvent) {
    const scroll = scrollShortcut(event, isEditing(event.target), singleKeyShortcuts());
    if (!scroll) return false;
    const content = $(".body") || $("main") || document.body;
    const style = getComputedStyle(content);
    let header = $(".itemhead") || $(".top");
    const top = header ? Math.max(0, header.getBoundingClientRect().bottom) : 0;
    const distance = scrollDistance(scroll, lineHeight(style.lineHeight, style.fontSize), window.innerHeight - top, preferences.values["scroll-amount"]);
    window.scrollBy({ top: distance, behavior: "instant" });
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
    if (!externalShortcutKey(key)) return false;
    const article = KIND === "item" ? $("article.item") : listRows().find(row => row.classList.contains("is-selected"));
    if (!article) return false;
    const title = $(".p-name", article);
    const target = externalShortcutTarget(key, {
      original: $<HTMLAnchorElement>(".u-bookmark-of", article)?.href ?? null,
      link: article.dataset.link || "",
      title: title ? title.textContent || "" : "",
      discussions: $$<HTMLAnchorElement>(".discussion[data-discussion]", article).map(function (link) {
        return { name: link.dataset.discussion || "", href: link.href };
      })
    }, (readerWindow.AGGR && readerWindow.AGGR.discussions) || []);
    if (!target) return false;
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
    if (event.key === "/") {
      event.preventDefault();
      waitingForGoto = false;
      clearTimeout(gotoTimer);
      openGlobalSearch();
      return;
    }
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
      if (!article) return;
      const target = articleNavigationTarget(direction, article.dataset, BASE);
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

  function reloadPositionKey() { return entryStateKey("reload-position", BASE); }
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

  function renderPullRefresh(next: PullPhase, distance: number, changed: boolean) {
    let root = document.documentElement;
    const indicator = $("#pull-refresh");
    let label = $("#pull-refresh-label");
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
    label.textContent = pullLabel(next);
  }

  function wirePullRefresh() {
    let root = document.documentElement;
    if (root.dataset.pullRefreshBound === "true") return;
    if (!PWA || !isInstalled() || !(navigator.maxTouchPoints > 0 || "ontouchstart" in window)) return;
    root.dataset.pullRefreshBound = "true";
    wireTouchPullRefresh({
      document, window,
      canStart: target => isInstalled() && !(window.scrollY > 0) && !$("dialog[open]") && !isEditing(target),
      render: renderPullRefresh,
      refresh() {
        showConnectionStatus("Refreshing for new items…", false, false);
        rememberReloadPosition();
        location.reload();
      }
    });
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
      syncPageHead(document.head, incoming);
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
        readerWindow.AGGR.entries = scopedEntries(build.entries, BASE);
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
    pageScope.add(() => articleHeader.dispose());
    // Every step is guarded: one failing enhancement is recorded and the rest of the page still boots.
    safely("boot:navigation", () => navigation.acceptPage());
    safely("boot:page-context", () => {
      let page = $("#aggr-page");
      if (!page) return;
      BASE = resolveRoot(page.dataset.root);
      KIND = page.dataset.kind || "";
      if (document.body.dataset.kind !== KIND) document.body.dataset.kind = KIND;
      $$("[data-site-navigation] [data-route]").forEach(function (link) {
        const target = new URL(link.dataset.route || "", BASE);
        const href = target.pathname + target.search + target.hash;
        if (link.getAttribute("href") !== href) link.setAttribute("href", href);
      });
      updateMenuSelection();
    });
    safely("boot:menu-navigation", () => wireMenuNavigation());
    safely("boot:pull-refresh", () => wirePullRefresh());
    safely("boot:preference-location", () => preferenceService.importLocation(KIND));
    safely("boot:preferences", () => {
      const preferencePanel = mountPreferences($("#swup") || document, preferenceService, { onShortcuts: shortcutHelp });
      pageScope.add(() => preferencePanel.dispose());
    });
    safely("boot:apply-preferences", () => applyPreferences(preferences.values, false));
    safely("boot:margin-notes", () => enhanceMarginNotes($("#swup") || document, marginNoteViewport, document));
    safely("boot:preview-media", () => enhancePreviewMedia($("#swup") || document));
    safely("boot:video-player", () => enhanceVideoPlayer());
    safely("boot:article-media", () => enhanceArticleMedia($("#swup") || document));
    safely("boot:selection-sharing", () => mountSelectionSharing($("#swup") || document, pageScope.signal));
    safely("boot:article-header", () => articleHeader.wire());
    safely("boot:article-swipe", () => {
      const article = $("article.item");
      if (!article) return;
      const scope = pageScope;
      article.setAttribute("data-swipe-navigation", "");
      scope.add(mountArticleSwipe(article, {
        previous: () => articleNavigationTarget("previous", article.dataset, BASE),
        next: () => articleNavigationTarget("next", article.dataset, BASE),
        navigate,
        isBusy: () => !!swup?.navigating || scope.signal.aborted,
      }));
      scope.add(() => article.removeAttribute("data-swipe-navigation"));
    });
    safely("boot:external-links", () => externalLinks(pageEpoch ? $("#swup") || document : document));
    safely("boot:search", () => fillSearch());
    safely("boot:list-cursor", () => { if (!searchController?.isActive()) restoreListCursor(restoreListFocus && !swup); });
    safely("boot:new-entries", () => showNewEntries($("#swup") || document));
  }

  window.addEventListener("appinstalled", function () {
    externalLinks(document);
    wirePullRefresh();
  });
  window.addEventListener("offline", updateConnectionStatus);
  window.addEventListener("online", updateConnectionStatus);

  if (window.performance && performance.getEntriesByType) {
    const navigationEntry = performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined;
    restoreListFocus = !!(navigationEntry && navigationEntry.type === "back_forward");
  }
  safely("new-entries", () => detectNewEntries());
  safely("boot", () => bootPage());
  safely("reload-position", () => restoreReloadPosition());
  window.addEventListener("pagehide", event => {
    if (event.persisted) return;
    void pageScope.dispose().catch(() => {});
    void appScope.dispose().catch(() => {});
  });
  const mobileBar = $(".mobile-tabs");
  if (mobileBar) safely("mobile-navigation", () => appScope.add(mountMobileNavigation(mobileBar, window)));
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
  const Swup = readerWindow.Swup;
  if (Swup) safely("swup", () => {
    readerWindow.swup = swup = new Swup({ containers: ["#swup"], cache: true, animationSelector: false, native: false, animateHistoryBrowsing: false });
    
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
      syncPageHead(document.head, visit && visit.to && visit.to.document);
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
  });
  safely("warm-page", () => warmPage());

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
    enhanceMarginNotes($("#swup") || document, marginNoteViewport, document);
    externalLinks($(".body") || document);
  };
  if (marginNoteViewport.addEventListener) marginNoteViewport.addEventListener("change", syncMarginNotes);
  else if (marginNoteViewport.addListener) marginNoteViewport.addListener(syncMarginNotes);
  safely("offline-client", () => offlineClient.start());
  safely("build-watcher", () => buildWatcher.start());
  setInterval(function () {
    if (document.visibilityState === "visible") formatTimes($("#swup") || document);
  }, 60 * 1000);
})();
