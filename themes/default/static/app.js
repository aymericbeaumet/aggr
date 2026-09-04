// aggr default theme. No build step, no dependencies.
(function () {
  "use strict";
  function resolveRoot(relative) { return new URL(relative || "./", window.location.href).href; }
  var script = document.querySelector("script[src$='assets/app.js']");
  var BASE = resolveRoot((window.AGGR && window.AGGR.base) || (script && script.getAttribute("src").slice(0, -"assets/app.js".length)) || "./");
  var KIND = (window.AGGR && window.AGGR.kind) || document.body.getAttribute("data-kind") || "";
  var PWA = window.AGGR ? window.AGGR.pwa !== false : true;
  var darkPreference = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)");
  var gotoTimer;
  var waitingForGoto = false;
  var pendingNewEntries = [];
  var newEntryTimer;
  var faviconState = { original: null, badged: null, loading: false, active: false };
  var PREFERENCE_KEYS = ["aggr:theme", "aggr:date-format"];
  var deferredInstallPrompt;
  var statusTimer;
  var PULL_THRESHOLD = 84;
  var PULL_MAX = 72;
  var PULL_HOLD = 48;
  var pullRefreshState = "idle";
  var pullStartX = 0;
  var pullStartY = 0;
  var pullResetTimer;

  function $(selector, root) { return (root || document).querySelector(selector); }
  function $$(selector, root) { return Array.prototype.slice.call((root || document).querySelectorAll(selector)); }
  function el(tag, attrs, children) {
    var node = document.createElement(tag);
    Object.keys(attrs || {}).forEach(function (key) {
      if (key === "text") node.textContent = attrs[key];
      else if (key === "html") node.innerHTML = attrs[key];
      else node.setAttribute(key, attrs[key]);
    });
    (children || []).forEach(function (child) { if (child) node.appendChild(child); });
    return node;
  }
  function ago(iso) {
    var timestamp = Date.parse(iso);
    if (isNaN(timestamp)) return null;
    var seconds = Math.max(0, Math.round((Date.now() - timestamp) / 1000));
    if (seconds < 60) return "just now";
    var minutes = Math.floor(seconds / 60);
    if (minutes < 60) return minutes + "m ago";
    var hours = Math.floor(minutes / 60);
    if (hours < 24) return hours + "h ago";
    var days = Math.ceil(hours / 24);
    if (days < 45) return days + "d ago";
    var months = Math.floor(days / 30);
    if (months < 18) return months + "mo ago";
    return Math.floor(days / 365) + "y ago";
  }
  function dateFormat() {
    var stored;
    try { stored = localStorage.getItem("aggr:date-format"); } catch (error) { /* private mode */ }
    return /^(relative|iso|local|local-time)$/.test(stored || "") ? stored : "relative";
  }
  function dateText(iso, format) {
    var timestamp = Date.parse(iso);
    if (isNaN(timestamp)) return null;
    var date = new Date(timestamp);
    if (format === "iso") return date.toISOString().slice(0, 10);
    if (format === "local") return new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(date);
    if (format === "local-time") return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(date);
    return ago(iso);
  }
  function formatTimes(root) {
    var format = dateFormat();
    $$("time[datetime]", root).forEach(function (time) {
      var exact = time.getAttribute("datetime");
      var text = dateText(exact, format);
      var timestamp = Date.parse(exact);
      if (text && !isNaN(timestamp)) {
        var local = new Intl.DateTimeFormat(undefined, {
          weekday: "long", year: "numeric", month: "long", day: "numeric",
          hour: "2-digit", minute: "2-digit", second: "2-digit"
        }).format(new Date(timestamp));
        time.title = local;
        time.setAttribute("aria-label", text + "; " + local);
        time.textContent = text;
      }
    });
    applyAgeBands(root);
  }

  function enhanceMarginNotes(root) {
    $$(".body", root).forEach(function (body) {
      if (body.dataset.marginNotesEnhanced === "true") return;
      var count = 0;
      $$(".footnote-ref a[data-footnote-ref]", body).forEach(function (reference) {
        var href = reference.getAttribute("href") || "";
        if (href.charAt(0) !== "#") return;
        var id = href.slice(1);
        try { id = decodeURIComponent(id); } catch (error) { /* keep the literal fragment */ }
        var definition = document.getElementById(id);
        if (!definition || !body.contains(definition)) return;

        var number = reference.textContent.trim();
        var marginId = (reference.id || id + "-reference-" + (count + 1)) + "-note";
        var note = el("aside", {
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
        var marker = el("span", { "class": "margin-note-number", "text": number + ". " });
        var firstParagraph = $("p", note);
        if (firstParagraph) firstParagraph.insertBefore(marker, firstParagraph.firstChild);
        else note.insertBefore(marker, note.firstChild);

        reference.removeAttribute("target");
        reference.removeAttribute("rel");
        reference.setAttribute("aria-describedby", note.id);
        reference.parentNode.insertAdjacentElement("afterend", note);
        reference.addEventListener("click", function (event) {
          if (!window.matchMedia("(min-width: 72.0625rem)").matches) return;
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

  function ageBand(iso) {
    var age = Math.max(0, Date.now() - Date.parse(iso));
    if (age < 60 * 60 * 1000) return "fresh";
    if (age < 3 * 60 * 60 * 1000) return "h1";
    if (age < 24 * 60 * 60 * 1000) return "h3";
    return "h24";
  }
  function applyAgeBands(root) {
    $$(".rows:not(.search-results)", root).forEach(function (list) {
      $$(".row", list).forEach(function (row) {
        var time = $(".meta time[datetime]", row);
        if (!time) return;
        var band = ageBand(time.getAttribute("datetime"));
        row.classList.remove("age-fresh", "age-h1", "age-h3", "age-h24");
        row.classList.add("age-" + band);
        row.dataset.age = band;
      });
    });
  }
  function setDateFormat(format) {
    var value = /^(relative|iso|local|local-time)$/.test(format || "") ? format : "relative";
    try { localStorage.setItem("aggr:date-format", value); } catch (error) { /* private mode */ }
    var picker = $("#date-format");
    if (picker) picker.value = value;
    formatTimes($("#swup") || document);
  }

  function externalLinks(root) {
    var installed = isInstalled();
    var scope = new URL(BASE);
    $$('a[href]', root).forEach(function (link) {
      try {
        var url = new URL(link.getAttribute('href'), document.baseURI);
        var outOfScope = url.origin !== scope.origin || url.pathname.indexOf(scope.pathname) !== 0;
        if (/^https?:$/.test(url.protocol) && outOfScope) {
          link.setAttribute("data-no-swup", "");
          if (installed) link.removeAttribute("target");
          else link.target = "_blank";
          link.relList.add("noopener", "noreferrer");
          var label = link.dataset.aggrExternalLabel || link.getAttribute("aria-label") || link.textContent.trim();
          var behavior = installed ? "external site" : "opens in a new tab";
          if (label) {
            link.dataset.aggrExternalLabel = label;
            link.setAttribute("aria-label", label + ", " + behavior);
          }
        }
      } catch (error) { /* an incomplete local link */ }
    });
  }

  function entryStateKey(name) {
    return "aggr:" + name + ":" + encodeURIComponent(new URL(BASE).pathname);
  }
  function readSessionList(key) {
    try {
      var value = sessionStorage.getItem(key);
      if (value === null) return null;
      var parsed = JSON.parse(value);
      return Array.isArray(parsed) ? parsed.filter(function (item) { return typeof item === "string"; }) : null;
    } catch (error) { return null; }
  }
  function readSessionValue(key) {
    try { return sessionStorage.getItem(key); } catch (error) { return null; }
  }
  function writeSessionList(key, value) {
    try { sessionStorage.setItem(key, JSON.stringify(value)); } catch (error) { /* private mode */ }
  }
  function writeSessionValue(key, value) {
    try { sessionStorage.setItem(key, value); } catch (error) { /* private mode */ }
  }
  function uniqueEntries(entries) {
    return entries.filter(function (entry, index) { return entries.indexOf(entry) === index; });
  }
  function currentRecentEntries() {
    var entries = window.AGGR && Array.isArray(window.AGGR.entries) ? window.AGGR.entries : [];
    var home = new URL(location.href).pathname === new URL(BASE).pathname;
    if (KIND === "river" && home) {
      entries = entries.concat($$(".rows:not(.search-results) .row[data-url]").map(function (row) {
        return row.dataset.url;
      }));
    }
    return uniqueEntries(entries.map(function (entry) {
      try { return new URL(entry, BASE).href; } catch (error) { return null; }
    }).filter(Boolean));
  }
  function detectNewEntries() {
    var seenKey = entryStateKey("last-seen-entry");
    var pendingKey = entryStateKey("new-entries");
    var current = currentRecentEntries();
    var previousHead = readSessionValue(seenKey);
    var pending = readSessionList(pendingKey) || [];
    if (previousHead && current.length) {
      var boundary = current.indexOf(previousHead);
      var additions = current.slice(0, boundary === -1 ? current.length : boundary);
      pending = uniqueEntries(pending.concat(additions));
    }
    if (current.length) writeSessionValue(seenKey, current[0]);
    writeSessionList(pendingKey, pending);
    pendingNewEntries = pending;
  }
  function updateFavicon(active) {
    var icon = $('link[rel~="icon"]');
    if (!icon) return;
    if (!faviconState.original) faviconState.original = icon.getAttribute("href");
    faviconState.active = active;
    if (!active) {
      icon.setAttribute("href", faviconState.original);
      return;
    }
    if (faviconState.badged) {
      icon.setAttribute("href", faviconState.badged);
      return;
    }
    if (faviconState.loading) return;
    faviconState.loading = true;
    var image = new Image();
    image.onload = function () {
      faviconState.loading = false;
      var size = Math.max(image.naturalWidth || 0, 32);
      var canvas = document.createElement("canvas");
      canvas.width = size;
      canvas.height = size;
      var context = canvas.getContext("2d");
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
    image.src = new URL(faviconState.original, document.baseURI).href;
  }
  function updateNewEntryIndicator(active) {
    document.title = document.title.replace(/^●\s+/, "");
    if (active) document.title = "● " + document.title;
    updateFavicon(active);
  }
  function acknowledgeNewEntries() {
    pendingNewEntries = [];
    try { sessionStorage.removeItem(entryStateKey("new-entries")); } catch (error) { /* private mode */ }
    $$(".row.is-new").forEach(function (row) { row.classList.remove("is-new"); });
    $$(".new-marker").forEach(function (marker) { marker.remove(); });
    updateNewEntryIndicator(false);
  }
  function showNewEntries(root) {
    updateNewEntryIndicator(pendingNewEntries.length > 0);
    if (!pendingNewEntries.length || document.visibilityState !== "visible") return;
    var highlighted = 0;
    $$(".rows:not(.search-results) .row[data-url]", root).forEach(function (row) {
      var entry;
      try { entry = new URL(row.dataset.url, BASE).href; } catch (error) { return; }
      if (pendingNewEntries.indexOf(entry) === -1) return;
      row.classList.add("is-new");
      var cell = $(".cell", row);
      if (cell && !$(".new-marker", cell)) {
        cell.insertBefore(el("span", { "class": "new-marker", "aria-label": "New item", text: "new" }), cell.firstChild);
      }
      highlighted += 1;
    });
    if (KIND === "river" && highlighted) {
      var announcer = $("#aggr-announcer");
      if (announcer) announcer.textContent = highlighted + (highlighted === 1 ? " new item" : " new items");
      clearTimeout(newEntryTimer);
      newEntryTimer = setTimeout(function () {
        if (KIND === "river" && document.visibilityState === "visible") acknowledgeNewEntries();
      }, 12 * 1000);
    }
  }

  var PAGE_HEAD_SELECTOR = [
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
  function syncPageHead(incoming) {
    if (!incoming || !incoming.head) return;
    $$(PAGE_HEAD_SELECTOR, document.head).forEach(function (node) { node.remove(); });
    $$(PAGE_HEAD_SELECTOR, incoming.head).forEach(function (node) {
      document.head.appendChild(document.importNode(node, true));
    });
  }

  function announceNavigation() {
    var announcer = $("#aggr-announcer");
    if (!announcer) return;
    announcer.textContent = "";
    requestAnimationFrame(function () { announcer.textContent = "Navigated to " + document.title; });
  }

  function setTheme(mode) {
    document.documentElement.dataset.theme = mode;
    try { localStorage.setItem("aggr:theme", mode); } catch (error) { /* private mode */ }
    var picker = $("#theme-mode");
    if (picker) picker.value = mode;
    var meta = $("#theme-color");
    if (meta) meta.setAttribute("content", getComputedStyle(document.documentElement).getPropertyValue("--nav-bg").trim());
  }
  function themeMode() {
    var stored;
    try { stored = localStorage.getItem("aggr:theme"); } catch (error) { /* private mode */ }
    setTheme(/^(auto|light|dark)$/.test(stored || "") ? stored : "auto");
  }

  function importState() {
    var url = new URL(location.href);
    var encoded = url.searchParams.get("aggr-state");
    if (!encoded) return;
    try {
      var base64 = encoded.replace(/-/g, "+").replace(/_/g, "/");
      while (base64.length % 4) base64 += "=";
      var state = JSON.parse(decodeURIComponent(escape(atob(base64))));
      PREFERENCE_KEYS.forEach(function (key) {
        if (Object.prototype.hasOwnProperty.call(state, key) && typeof state[key] === "string") {
          localStorage.setItem(key, state[key]);
        }
      });
      url.searchParams.delete("aggr-state");
      history.replaceState(null, "", url);
    } catch (error) { /* malformed share URL */ }
  }
  function copyState() {
    var state = {};
    PREFERENCE_KEYS.forEach(function (key) {
      var value = localStorage.getItem(key);
      if (value !== null) state[key] = value;
    });
    var encoded = btoa(unescape(encodeURIComponent(JSON.stringify(state)))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    var url = new URL(location.href);
    url.searchParams.set("aggr-state", encoded);
    var status = $("#copy-state-status");
    navigator.clipboard.writeText(url.toString()).then(function () {
      if (status) status.textContent = "copied";
      setTimeout(function () { if (status) status.textContent = ""; }, 1600);
    }).catch(function () {
      if (status) status.textContent = "could not access the clipboard";
    });
  }

  function renderResult(entry, display, rank) {
    var page = BASE + entry.url.replace(/^\/+/, "");
    display = display || {};
    var original = display.original || page;
    var date = entry.meta.date || "";
    var meta = [date ? el("time", { "class": "dt-published", datetime: date, title: date, text: date.slice(0, 10) }) : null, document.createTextNode(" · "), el("a", { "class": "u-bookmark-of", href: original, target: "_blank", rel: "external noopener noreferrer via", text: "original" })];
    var discussions = display.discussions || ((window.AGGR && window.AGGR.discussions) || []);
    discussions.forEach(function (discussion) {
      var url = discussion.url.replace("{url}", encodeURIComponent(original)).replace("{title}", encodeURIComponent(entry.meta.title || ""));
      meta.push(document.createTextNode(" · "));
      meta.push(el("a", {
        "class": "discussion" + (discussion.found ? " is-found" : ""),
        href: url, target: "_blank", rel: "noopener noreferrer",
        "aria-label": discussion.found ? discussion.name + ", matching discussion found" : discussion.name,
        text: discussion.name.toLowerCase().replace(/\s+/g, "")
      }));
    });
    var excerpt = el("div", { "class": "search-excerpt" });
    if (entry.excerpt) excerpt.innerHTML = entry.excerpt;
    else excerpt.textContent = display.excerpt || "";
    return el("li", { "class": "row h-entry", "data-url": page, "data-link": original }, [
      el("a", { "class": "u-uid", href: page, hidden: "" }),
      el("span", { "class": "rank", text: rank + "." }),
      el("div", { "class": "cell" }, [
        el("a", { "class": "title p-name u-url", href: page, text: entry.meta.title }),
        display.domain ? el("a", { "class": "domain", href: BASE + "sources/" + display.source_slug + "/", text: "(" + display.domain + ")" }) : null,
        excerpt,
        el("div", { "class": "meta" }, meta.filter(Boolean))
      ])
    ]);
  }

  var pagefind;
  var pagefindData = new Map();
  function loadPagefind() {
    if (!pagefind) {
      pagefind = import(new URL("pagefind/pagefind.js", BASE).href).then(async function (api) {
        await api.options({
          basePath: new URL("pagefind/", BASE).pathname,
          excerptLength: 28,
          ranking: {
            termFrequency: 0.65,
            termSimilarity: 1,
            pageLength: 0.35,
            termSaturation: 0.8,
            metaWeights: {
              title: 12,
              source: 2,
              date: 0,
              aggr_display: 0
            }
          }
        });
        await api.init();
        return api;
      });
    }
    return pagefind;
  }
  function searchDisplay(data) {
    var meta = data.meta || {};
    try {
      var hex = meta.aggr_display || "";
      var bytes = new Uint8Array(hex.length / 2);
      for (var i = 0; i < bytes.length; i += 1) bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
      return JSON.parse(new TextDecoder().decode(bytes));
    } catch (error) { return {}; }
  }
  function resultData(result) {
    var key = result.id || result.url;
    if (!pagefindData.has(key)) pagefindData.set(key, result.data());
    return pagefindData.get(key);
  }
  function fillSearch() {
    var list = $("#list");
    var input = $("#q");
    if (!list || !input) return;
    var form = $("#search-form");
    var category = $("#category-filter");
    var tag = $("#tag-filter");
    var sort = $("#search-sort");
    var count = $("#count");
    var status = $("#search-status");
    var empty = $("#empty");
    var initialQuery = new URL(location.href).searchParams.get("q");
    var initialUrl = new URL(location.href);
    if (!input.value && initialQuery) input.value = initialQuery;
    if (category && initialUrl.searchParams.has("category")) category.value = initialUrl.searchParams.get("category");
    if (initialUrl.searchParams.has("tag")) tag.value = initialUrl.searchParams.get("tag");
    if (initialUrl.searchParams.get("sort") === "newest") sort.value = "newest";
    var generation = 0;
    function show(rows, total, searched) {
      var fragment = document.createDocumentFragment();
      rows.forEach(function (entry, i) { fragment.appendChild(renderResult(entry.data, entry.display, i + 1)); });
      list.replaceChildren(fragment);
      externalLinks(list);
      list.setAttribute("aria-busy", "false");
      empty.hidden = rows.length > 0;
      empty.textContent = searched ? "No matching items." : "Type to search the most recent items.";
      var label = total + (total === 1 ? " result" : " results");
      count.textContent = total > 999 ? "999+" : String(total);
      count.style.visibility = searched ? "visible" : "hidden";
      if (status) status.textContent = searched ? label : "";
      formatTimes(list);
    }
    async function render() {
      var current = ++generation;
      var query = input.value.trim();
      var filters = {};
      if (category && category.value) filters.category = category.value;
      if (tag.value) filters.tag = tag.value;
      var searched = !!query || !!Object.keys(filters).length;
      if (!searched) { show([], 0, false); return; }
      list.setAttribute("aria-busy", "true");
      try {
        var api = await loadPagefind();
        var options = { filters: filters };
        if (sort.value === "newest") options.sort = { date: "desc" };
        var found = query && api.debouncedSearch
          ? await api.debouncedSearch(query, options, 25)
          : await api.search(query || null, options);
        if (!found || current !== generation) return;
        var results = found.results.slice(0, 40);
        var first = results.slice(0, 12);
        var rows = await Promise.all(first.map(async function (result) {
          var data = await resultData(result);
          return { data: data, display: searchDisplay(data) };
        }));
        if (current !== generation) return;
        show(rows, found.results.length, true);
        if (results.length > first.length) {
          var rest = await Promise.all(results.slice(first.length).map(async function (result) {
            var data = await resultData(result);
            return { data: data, display: searchDisplay(data) };
          }));
          if (current === generation) show(rows.concat(rest), found.results.length, true);
        }
      } catch (error) {
        if (current === generation) {
          show([], 0, true);
          empty.textContent = "Search is unavailable right now.";
        }
      }
    }
    function updateLocation() {
      var url = new URL(location.href);
      var query = input.value.trim();
      if (query) url.searchParams.set("q", query);
      else url.searchParams.delete("q");
      if (category && category.value) url.searchParams.set("category", category.value);
      else url.searchParams.delete("category");
      if (tag.value) url.searchParams.set("tag", tag.value);
      else url.searchParams.delete("tag");
      if (sort.value === "newest") url.searchParams.set("sort", "newest");
      else url.searchParams.delete("sort");
      history.replaceState(history.state, "", url);
    }
    function schedule() {
      updateLocation();
      render();
    }
    count.style.visibility = "hidden";
    input.addEventListener("focus", loadPagefind, { once: true });
    input.addEventListener("input", schedule);
    if (category) category.addEventListener("change", schedule);
    tag.addEventListener("change", schedule);
    sort.addEventListener("change", schedule);
    if (form) form.addEventListener("submit", async function (event) {
      event.preventDefault();
      await render();
      openFirstResult();
    });
    if (document.activeElement === input) loadPagefind();
    if (input.value.trim() || (category && category.value) || tag.value) render();
  }

  function fillDirectory() {
    var input = $("[data-directory-filter]");
    var table = $("[data-directory]");
    if (!input || !table || input.dataset.bound === "true") return;
    input.dataset.bound = "true";
    var rows = $$('[data-directory-entry]', table);
    var count = $("#directory-count");
    var empty = $("[data-directory-empty]");
    var form = input.closest("form");
    function filter() {
      var tokens = input.value.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
      var visible = 0;
      rows.forEach(function (row) {
        var haystack = row.textContent.toLocaleLowerCase();
        var match = tokens.every(function (token) { return haystack.indexOf(token) !== -1; });
        row.hidden = !match;
        if (match) visible += 1;
      });
      if (count) count.textContent = String(visible);
      if (empty) empty.hidden = visible !== 0;
    }
    input.addEventListener("input", filter);
    if (form) form.addEventListener("submit", function (event) {
      event.preventDefault();
      openFirstResult();
    });
  }

  function navigate(target) {
    var destination = new URL(target, document.baseURI).href;
    if (window.swup) window.swup.navigate(destination);
    else location.href = destination;
  }
  function firstResultLink() {
    var row = KIND === "search"
      ? $("#list .row")
      : $("[data-directory-entry]:not([hidden])");
    return row && ($("a.title", row) || $("a:not([target])", row) || $("a", row));
  }
  function openFirstResult() {
    var link = firstResultLink();
    if (!link) return false;
    navigate(link.href);
    return true;
  }
  function focusPageSearch(select) {
    var input = $("[data-page-search]");
    if (!input) return false;
    if (input.id === "q") {
      loadPagefind().catch(function () { /* the search page reports an unavailable index */ });
    }
    input.focus({ preventScroll: true });
    if (select && input.select) input.select();
    return true;
  }
  function wireMenuNavigation() {
    $$(".nav a[data-route]:not([target])").forEach(function (link) {
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
  function shortcutHelp() {
    var dialog = $("#shortcut-help");
    if (!dialog) return;
    if (dialog.open) dialog.close();
    else if (dialog.showModal) dialog.showModal();
    else dialog.setAttribute("open", "");
  }
  function wireShortcutHelp() {
    var dialog = $("#shortcut-help");
    if (!dialog || dialog.dataset.bound === "true") return;
    dialog.dataset.bound = "true";
    dialog.addEventListener("click", function (event) {
      if (event.target === dialog) dialog.close();
    });
  }
  function beginGoto() {
    waitingForGoto = true;
    clearTimeout(gotoTimer);
    gotoTimer = setTimeout(function () { waitingForGoto = false; }, 1200);
  }
  function finishGoto(key) {
    if (!waitingForGoto) return false;
    waitingForGoto = false;
    clearTimeout(gotoTimer);
    var routes = { f: "", i: "", "/": "search/", c: "categories/", t: "tags/", s: "sources/", p: "preferences/" };
    if (!(window.AGGR && window.AGGR.hasCategories)) delete routes.c;
    if (Object.prototype.hasOwnProperty.call(routes, key)) {
      navigate(new URL(routes[key], BASE).href);
      return true;
    }
    if (/^[1-9]$/.test(key)) {
      var entries = (window.AGGR && window.AGGR.entries) || [];
      var entry = entries[Number(key) - 1];
      if (entry) {
        navigate(new URL(entry, BASE).href);
        return true;
      }
    }
    return false;
  }
  function isEditing(target) {
    return target instanceof Element && !!target.closest("input, textarea, select, button, a, [contenteditable=true]");
  }
  function navigationDirection(key) {
    var normalized = key.length === 1 ? key.toLowerCase() : key;
    if (["ArrowLeft", "h", "k"].indexOf(normalized) !== -1) return "previous";
    if (["ArrowRight", "l", "j"].indexOf(normalized) !== -1) return "next";
    return null;
  }
  function articleScroll(event) {
    if (KIND !== "item" || event.altKey || event.metaKey || isEditing(event.target)) return false;
    var key = event.key.toLowerCase();
    if (key !== "d" && key !== "u") return false;
    var distance = Math.max(180, Math.min(420, window.innerHeight * 0.42));
    window.scrollBy(0, key === "d" ? distance : -distance);
    return true;
  }
  document.addEventListener("keydown", function (event) {
    if (!event.altKey && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      if (KIND === "search" && focusPageSearch(true)) {
        return;
      } else {
        var search = new URL("search/", BASE).href;
        if (window.swup) window.swup.navigate(search);
        else location.href = search;
      }
      return;
    }
    if (articleScroll(event)) {
      event.preventDefault();
      return;
    }
    if (event.altKey || event.ctrlKey || event.metaKey || isEditing(event.target)) return;
    if (event.key === "?") {
      event.preventDefault();
      shortcutHelp();
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
    if (event.key === "/") {
      if (focusPageSearch(false)) event.preventDefault();
      return;
    }
    var direction = navigationDirection(event.key);
    if (KIND === "river" && direction === "next") {
      var first = $(".rows .row");
      if (!first || !first.dataset.url) return;
      event.preventDefault();
      navigate(first.dataset.url);
      return;
    }
    if (KIND === "item" && direction) {
      var article = $("article.item");
      var target = article && (article.dataset[direction === "next" ? "nextUrl" : "previousUrl"] || (direction === "previous" ? BASE : null));
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
      || window.navigator.standalone === true;
  }

  function updatePwaControls(message) {
    var install = $("#install-app");
    var guidance = $("#install-guidance");
    var status = $("#pwa-status");
    var installed = isInstalled();
    if (guidance) {
      guidance.textContent = installed
        ? "aggr is running as an installed app."
        : "Use your browser’s Install or Add to Home Screen action. A direct install button appears here when the browser provides one.";
    }
    if (install) install.hidden = installed || !deferredInstallPrompt;
    if (status) status.textContent = message || "";
  }

  function wirePwaControls() {
    var install = $("#install-app");
    if (install && install.dataset.bound !== "true") {
      install.dataset.bound = "true";
      install.addEventListener("click", function () {
        if (!deferredInstallPrompt) return;
        var prompt = deferredInstallPrompt;
        deferredInstallPrompt = null;
        prompt.prompt();
        prompt.userChoice.then(function (choice) {
          updatePwaControls(choice.outcome === "accepted" ? "Install accepted." : "Install dismissed.");
        }).catch(function () { updatePwaControls(); });
      });
    }
    updatePwaControls();
  }

  function reloadPositionKey() { return entryStateKey("reload-position"); }
  function rememberReloadPosition() {
    try {
      var active = document.activeElement;
      var dialog = $("#shortcut-help");
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
      var raw = sessionStorage.getItem(reloadPositionKey());
      sessionStorage.removeItem(reloadPositionKey());
      if (!raw) return;
      var position = JSON.parse(raw);
      if (position.url !== location.href) return;
      requestAnimationFrame(function () {
        requestAnimationFrame(function () {
          var dialog = $("#shortcut-help");
          if (position.shortcutHelp && dialog && dialog.showModal && !dialog.open) dialog.showModal();
          var focus = position.focus && document.getElementById(position.focus);
          if (focus && focus.focus) focus.focus({ preventScroll: true });
          window.scrollTo(position.x || 0, position.y || 0);
        });
      });
    } catch (error) { /* malformed or unavailable session state */ }
  }

  function showConnectionStatus(message, retry, temporary) {
    var bar = $("#connection-status");
    var text = $("#connection-status-message");
    var button = $("#connection-retry");
    if (!bar || !text || !button) return;
    clearTimeout(statusTimer);
    text.textContent = message;
    button.hidden = !retry;
    bar.hidden = false;
    if (temporary) statusTimer = setTimeout(function () { bar.hidden = true; }, 2200);
  }

  function updateConnectionStatus() {
    var bar = $("#connection-status");
    if (!bar) return;
    if (!navigator.onLine) showConnectionStatus("Offline — showing saved pages.", true, false);
    else bar.hidden = true;
  }

  function setPullRefreshState(next, distance) {
    var root = document.documentElement;
    var indicator = $("#pull-refresh");
    var label = $("#pull-refresh-label");
    var changed = pullRefreshState !== next;
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
    var root = document.documentElement;
    if (root.dataset.pullRefreshBound === "true") return;
    if (!PWA || !(navigator.maxTouchPoints > 0 || "ontouchstart" in window)) return;
    root.dataset.pullRefreshBound = "true";

    document.addEventListener("touchstart", function (event) {
      if (pullRefreshState === "refreshing" || event.touches.length !== 1) return;
      if (window.scrollY > 0 || $("dialog[open]")) return;
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
      var deltaX = Math.abs(event.touches[0].clientX - pullStartX);
      var deltaY = event.touches[0].clientY - pullStartY;
      if (deltaY <= 0 || deltaX > deltaY) {
        settlePullRefresh();
        return;
      }
      if (deltaY < 6) return;
      event.preventDefault();
      var distance = Math.min(PULL_MAX, Math.round(deltaY * 0.55));
      if (deltaY >= PULL_THRESHOLD) setPullRefreshState("armed", distance);
      else setPullRefreshState("pulling", distance);
    }, { passive: false });

    document.addEventListener("touchend", function () {
      if (pullRefreshState === "armed") {
        clearTimeout(pullResetTimer);
        setPullRefreshState("refreshing", PULL_HOLD);
        showConnectionStatus("Refreshing for new items…", false, false);
        setTimeout(function () { location.reload(); }, 140);
      } else {
        settlePullRefresh();
      }
    }, { passive: true });
    document.addEventListener("touchcancel", settlePullRefresh, { passive: true });
  }

  function wirePersistentControls() {
    var refresh = $("#refresh-page");
    if (refresh && refresh.dataset.bound !== "true") {
      refresh.dataset.bound = "true";
      refresh.addEventListener("click", function (event) {
        event.preventDefault();
        showConnectionStatus("Refreshing for new items…", false, false);
        location.reload();
      });
    }
    var retry = $("#connection-retry");
    if (retry && retry.dataset.bound !== "true") {
      retry.dataset.bound = "true";
      retry.addEventListener("click", function () { location.reload(); });
    }
    updateConnectionStatus();
  }

  function serviceWorker() {
    if (!("serviceWorker" in navigator) || location.protocol === "file:") return;
    if (!PWA) {
      navigator.serviceWorker.getRegistration(BASE).then(function (registration) {
        if (registration) registration.unregister();
      });
      if ("caches" in window) {
        var namespace = "aggr:" + encodeURIComponent(new URL(BASE).pathname) + ":";
        caches.keys().then(function (names) {
          names.filter(function (name) { return name.indexOf(namespace) === 0; })
            .forEach(function (name) { caches.delete(name); });
        });
      }
      return;
    }
    if (KIND === "html") return;
    var UPDATE_INTERVAL = 60 * 1000;
    var controlled = !!navigator.serviceWorker.controller;
    var reloading = false;
    navigator.serviceWorker.addEventListener("controllerchange", function () {
      var nextController = navigator.serviceWorker.controller;
      if (controlled && nextController && !reloading) {
        reloading = true;
        rememberReloadPosition();
        showConnectionStatus("Updated — refreshing…", false, false);
        location.reload();
      }
      controlled = !!nextController;
    });
    navigator.serviceWorker.register(BASE + "sw.js", { updateViaCache: "none" }).then(function (registration) {
      var lastCheck = Date.now();
      function checkForUpdate() {
        if (!navigator.onLine) return;
        lastCheck = Date.now();
        registration.update().catch(function () { /* offline */ });
      }
      setInterval(checkForUpdate, UPDATE_INTERVAL);
      document.addEventListener("visibilitychange", function () {
        if (document.visibilityState === "visible") checkForUpdate();
      });
      window.addEventListener("online", checkForUpdate);
      window.addEventListener("pageshow", function (event) {
        if (event.persisted || Date.now() - lastCheck >= UPDATE_INTERVAL) checkForUpdate();
      });
    }).catch(function () { /* insecure context */ });
  }

  function bootPage() {
    var page = $("#aggr-page");
    if (page) {
      BASE = resolveRoot(page.dataset.root);
      KIND = page.dataset.kind;
      $("#aggr-base").setAttribute("href", BASE);
      document.body.dataset.kind = KIND;
      $$(".nav [data-route]").forEach(function (link) {
        link.setAttribute("href", new URL(link.dataset.route, document.baseURI).pathname);
      });
      $$(".nav [data-kinds]").forEach(function (link) {
        var active = link.dataset.kinds.split(/\s+/).indexOf(KIND) !== -1;
        if (active) link.setAttribute("aria-current", "page");
        else link.removeAttribute("aria-current");
      });
      var active = $(".nav [aria-current=page]");
      if (active) active.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
    wireMenuNavigation();
    wireShortcutHelp();
    wirePersistentControls();
    wireTouchPullRefresh();
    wirePwaControls();
    var picker = $("#theme-mode");
    if (picker) picker.addEventListener("change", function () { setTheme(picker.value); });
    var datePicker = $("#date-format");
    if (datePicker) datePicker.addEventListener("change", function () { setDateFormat(datePicker.value); });
    var copy = $("#copy-state");
    if (copy) copy.addEventListener("click", copyState);
    themeMode();
    setDateFormat(dateFormat());
    enhanceMarginNotes($("#swup") || document);
    externalLinks(document);
    fillSearch();
    fillDirectory();
    showNewEntries($("#swup") || document);
  }

  window.addEventListener("beforeinstallprompt", function (event) {
    event.preventDefault();
    deferredInstallPrompt = event;
    updatePwaControls();
  });
  window.addEventListener("appinstalled", function () {
    deferredInstallPrompt = null;
    updatePwaControls("aggr was installed.");
    externalLinks(document);
  });
  window.addEventListener("offline", updateConnectionStatus);
  window.addEventListener("online", function () {
    showConnectionStatus("Back online — checking for updates…", false, true);
  });

  importState();
  detectNewEntries();
  bootPage();
  restoreReloadPosition();
  document.addEventListener("visibilitychange", function () {
    if (document.visibilityState === "visible") showNewEntries($("#swup") || document);
  });
  var searchLink = $(".nav [data-route='search/']");
  if (searchLink) {
    ["pointerenter", "focus", "touchstart"].forEach(function (eventName) {
      searchLink.addEventListener(eventName, function () {
        loadPagefind().catch(function () { /* the search page reports an unavailable index */ });
      }, { once: true, passive: true });
    });
  }
  if (window.Swup) {
    window.swup = new window.Swup({ containers: ["#swup"], cache: true, animationSelector: false });
    window.swup.hooks.before("content:replace", function (visit) {
      syncPageHead(visit && visit.to && visit.to.document);
    });
    window.swup.hooks.on("page:view", function () {
      bootPage();
      announceNavigation();
      var target = KIND === "search" ? $("#q") : $("#swup");
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
  if (darkPreference) {
    var syncAutoTheme = function () {
      if (document.documentElement.dataset.theme === "auto") setTheme("auto");
    };
    if (darkPreference.addEventListener) darkPreference.addEventListener("change", syncAutoTheme);
    else if (darkPreference.addListener) darkPreference.addListener(syncAutoTheme);
  }
  serviceWorker();
  setInterval(function () { formatTimes($("#swup") || document); }, 60 * 1000);
})();
