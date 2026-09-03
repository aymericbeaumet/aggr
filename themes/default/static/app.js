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
    $$('a[href]', root).forEach(function (link) {
      try {
        var url = new URL(link.getAttribute('href'), document.baseURI);
        if (/^https?:$/.test(url.protocol) && url.origin !== location.origin) {
          link.target = '_blank';
          link.rel = 'noopener noreferrer';
        }
      } catch (error) { /* an incomplete local link */ }
    });
  }

  var PAGE_HEAD_SELECTOR = [
    'meta[name="description"]',
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
    'link[rel="via"]',
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
      Object.keys(state).forEach(function (key) { localStorage.setItem(key, state[key]); });
      url.searchParams.delete("aggr-state");
      history.replaceState(null, "", url);
    } catch (error) { /* malformed share URL */ }
  }
  function copyState() {
    var state = {};
    for (var i = 0; i < localStorage.length; i += 1) {
      var key = localStorage.key(i);
      state[key] = localStorage.getItem(key);
    }
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
              title: 9,
              source: 3,
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
    if (!input.value && initialQuery) input.value = initialQuery;
    var generation = 0;
    function show(rows, total, searched) {
      var fragment = document.createDocumentFragment();
      rows.forEach(function (entry, i) { fragment.appendChild(renderResult(entry.data, entry.display, i + 1)); });
      list.replaceChildren(fragment);
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
      if (category.value) filters.category = category.value;
      if (tag.value) filters.tag = tag.value;
      var searched = !!query || !!Object.keys(filters).length;
      if (!searched) { show([], 0, false); return; }
      list.setAttribute("aria-busy", "true");
      try {
        var api = await loadPagefind();
        var options = { filters: filters };
        if (sort.value === "newest") options.sort = { date: "desc" };
        var found = query && api.debouncedSearch
          ? await api.debouncedSearch(query, options, 45)
          : await api.search(query || null, options);
        if (!found || current !== generation) return;
        var results = found.results.slice(0, 40);
        var first = results.slice(0, 12);
        var rows = await Promise.all(first.map(async function (result) {
          var data = await result.data();
          return { data: data, display: searchDisplay(data) };
        }));
        if (current !== generation) return;
        show(rows, found.results.length, true);
        if (results.length > first.length) {
          var rest = await Promise.all(results.slice(first.length).map(async function (result) {
            var data = await result.data();
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
    function schedule() {
      var url = new URL(location.href);
      var query = input.value.trim();
      if (query) url.searchParams.set("q", query);
      else url.searchParams.delete("q");
      history.replaceState(history.state, "", url);
      render();
    }
    count.style.visibility = "hidden";
    input.addEventListener("focus", loadPagefind, { once: true });
    input.addEventListener("input", schedule);
    category.addEventListener("change", render);
    tag.addEventListener("change", render);
    sort.addEventListener("change", render);
    if (form) form.addEventListener("submit", function (event) { event.preventDefault(); render(); });
    if (document.activeElement === input) loadPagefind();
    if (input.value.trim() || category.value || tag.value) render();
  }

  function navigate(target) {
    var destination = new URL(target, document.baseURI).href;
    if (window.swup) window.swup.navigate(destination);
    else location.href = destination;
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
    var routes = { f: "", c: "categories/", t: "tags/", s: "sources/", p: "preferences/" };
    if (!Object.prototype.hasOwnProperty.call(routes, key)) return false;
    navigate(new URL(routes[key], BASE).href);
    return true;
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
  document.addEventListener("keydown", function (event) {
    if (!event.altKey && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      loadPagefind().catch(function () { /* the search page reports an unavailable index */ });
      var input = $("#q");
      if (KIND === "search" && input) {
        input.focus({ preventScroll: true });
        input.select();
      } else {
        var search = new URL("search/", BASE).href;
        if (window.swup) window.swup.navigate(search);
        else location.href = search;
      }
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
      event.preventDefault();
      var query = $("#q");
      if (KIND === "search" && query) query.focus({ preventScroll: true });
      else navigate(new URL("search/", BASE).href);
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

  function toast(text, action, onAction) {
    var button = el("button", { type: "button", "class": "linkish", text: action });
    button.addEventListener("click", onAction);
    document.body.appendChild(el("div", { "class": "toast", role: "status" }, [document.createTextNode(text), button]));
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
    var controlled = !!navigator.serviceWorker.controller;
    navigator.serviceWorker.addEventListener("controllerchange", function () {
      if (controlled) toast("Updated — ", "reload", function () { location.reload(); });
      controlled = true;
    });
    navigator.serviceWorker.register(BASE + "sw.js").catch(function () { /* insecure context */ });
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
    var picker = $("#theme-mode");
    if (picker) picker.addEventListener("change", function () { setTheme(picker.value); });
    var datePicker = $("#date-format");
    if (datePicker) datePicker.addEventListener("change", function () { setDateFormat(datePicker.value); });
    var copy = $("#copy-state");
    if (copy) copy.addEventListener("click", copyState);
    themeMode();
    setDateFormat(dateFormat());
    externalLinks($("#swup") || document);
    fillSearch();
  }

  importState();
  bootPage();
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
