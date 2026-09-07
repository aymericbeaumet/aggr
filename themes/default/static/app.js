// aggr default theme. No build step, no dependencies.
(function () {
  "use strict";
  function resolveRoot(relative) { return new URL(relative || "./", window.location.href).href; }
  var script = document.querySelector("script[src$='assets/app.js']");
  var BASE = resolveRoot((window.AGGR && window.AGGR.base) || (script && script.getAttribute("src").slice(0, -"assets/app.js".length)) || "./");
  var baseElement = document.querySelector("#aggr-base");
  if (baseElement) baseElement.href = BASE;
  var KIND = (window.AGGR && window.AGGR.kind) || document.body.getAttribute("data-kind") || "";
  var PWA = window.AGGR ? window.AGGR.pwa !== false : true;
  var darkPreference = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)");
  var motionPreference = window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)");
  var pageEpoch = 0;
  var currentPageUrl = location.href;
  var searchFocusRequested = false;
  var restoreListFocus = false;
  var updateState = "current";
  var gotoTimer;
  var waitingForGoto = false;
  var pendingNewEntries = [];
  var newEntryTimer;
  var faviconState = { original: null, badged: null, loading: false, active: false };
  var preferences = window.AGGRPreferences;
  var pendingPreferences = null;
  var preferenceImportMessage = "";
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
    return preferences.values["date-format"];
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

  function previewMedia(preview) {
    if (!preview) return null;
    var media = el("span", { "class": "preview-media" }, [
      el("img", {
        "class": "preview-image",
        src: new URL(preview.url, BASE).href,
        width: preview.width,
        height: preview.height,
        alt: preview.alt || "",
        loading: "lazy",
        decoding: "async"
      })
    ]);
    if (typeof preview.color === "string" && /^#[0-9a-f]{6}$/i.test(preview.color)) {
      media.style.setProperty("--preview-color", preview.color);
    }
    return media;
  }

  function enhancePreviewMedia(root) {
    $$(".preview-media", root).forEach(function (media) {
      if (media.dataset.previewBound === "true") return;
      var image = $(".preview-image", media);
      if (!image) return;
      media.dataset.previewBound = "true";
      if (image.complete) return;
      media.classList.add("is-loading");
      image.addEventListener("load", function () { media.classList.add("is-loaded"); }, { once: true });
      image.addEventListener("error", function () { media.classList.add("is-loaded", "is-error"); }, { once: true });
    });
  }

  function enhanceArticleMedia(root) {
    $$(".article-picture", root).forEach(function (media) {
      if (media.dataset.articleMediaBound === "true") return;
      var image = $(".progressive-image", media);
      if (!image) return;
      media.dataset.articleMediaBound = "true";

      function showPlaceholder() {
        var source = media.dataset.placeholder;
        if (!source || media.style.getPropertyValue("--image-preview")) return;
        var url = new URL(source, document.baseURI).href;
        media.style.setProperty("--image-preview", "url(" + JSON.stringify(url) + ")");
      }
      function loaded() {
        media.classList.add("is-loaded");
        media.removeAttribute("aria-busy");
      }
      function failed() {
        media.classList.remove("is-loading");
        media.classList.add("is-error");
        media.removeAttribute("aria-busy");
      }

      if (image.complete) {
        if (image.naturalWidth) loaded();
        else failed();
        return;
      }
      media.classList.add("is-loading");
      media.setAttribute("aria-busy", "true");
      if (image.loading !== "lazy" || !("IntersectionObserver" in window)) {
        showPlaceholder();
      } else {
        var observer = new IntersectionObserver(function (entries) {
          if (!entries.some(function (entry) { return entry.isIntersecting; })) return;
          observer.disconnect();
          showPlaceholder();
        }, { rootMargin: "480px" });
        observer.observe(media);
      }
      image.addEventListener("load", function () {
        if (typeof image.decode === "function") image.decode().catch(function () {}).then(loaded);
        else loaded();
      }, { once: true });
      image.addEventListener("error", failed, { once: true });
    });
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
  function acknowledgeNewEntries(entries) {
    var acknowledged = new Set(entries || []);
    pendingNewEntries = pendingNewEntries.filter(function (entry) { return !acknowledged.has(entry); });
    if (pendingNewEntries.length) writeSessionList(entryStateKey("new-entries"), pendingNewEntries);
    else {
      try { sessionStorage.removeItem(entryStateKey("new-entries")); } catch (error) { /* private mode */ }
    }
    $$(".row.is-new").forEach(function (row) {
      var entry;
      try { entry = new URL(row.dataset.url, BASE).href; } catch (error) { return; }
      if (acknowledged.has(entry)) row.classList.remove("is-new");
    });
    updateNewEntryIndicator(pendingNewEntries.length > 0);
  }
  function showNewEntries(root) {
    updateNewEntryIndicator(pendingNewEntries.length > 0);
    if (!pendingNewEntries.length || document.visibilityState !== "visible") return;
    var highlighted = [];
    $$(".rows:not(.search-results) .row[data-url]", root).forEach(function (row) {
      if (row.hidden) return;
      var entry;
      try { entry = new URL(row.dataset.url, BASE).href; } catch (error) { return; }
      if (pendingNewEntries.indexOf(entry) === -1) return;
      row.classList.add("is-new");
      highlighted.push(entry);
    });
    if (KIND === "river" && highlighted.length) {
      var announcer = $("#aggr-announcer");
      if (announcer) announcer.textContent = highlighted.length + (highlighted.length === 1 ? " new item" : " new items");
      clearTimeout(newEntryTimer);
      newEntryTimer = setTimeout(function () {
        if (KIND === "river" && document.visibilityState === "visible") acknowledgeNewEntries(highlighted);
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

  function singleKeyShortcuts() {
    return preferences.values["single-key-shortcuts"];
  }
  function nativeMotionEnabled() {
    return !!document.startViewTransition && preferences.values.motion !== "off" && !(motionPreference && motionPreference.matches);
  }
  function syncMotion() {
    if (window.swup && window.swup.options) {
      window.swup.options.native = nativeMotionEnabled();
      document.documentElement.classList.toggle("swup-native", window.swup.options.native);
    }
  }
  function preferenceStatus(message) {
    var status = $("#preferences-status");
    if (status) status.textContent = message;
  }
  function applyPreferences(state, persist) {
    var saved = true;
    Object.keys(state).forEach(function (key) {
      if (!preferences.valid(key, state[key])) return;
      preferences.values[key] = state[key];
      if (persist) {
        try { localStorage.setItem("aggr:" + key, String(state[key])); } catch (error) { saved = false; }
      }
      var attribute = preferences.schema[key].attribute;
      if (attribute) document.documentElement.dataset[attribute] = state[key];
    });
    $$("[data-preference]").forEach(function (control) {
      var value = preferences.values[control.dataset.preference];
      if (control.type === "checkbox") control.checked = value;
      else control.value = value;
    });
    var meta = $("#theme-color");
    if (meta) meta.setAttribute("content", getComputedStyle(document.documentElement).getPropertyValue("--nav-bg").trim());
    formatTimes($("#swup") || document);
    applyFeedPagination();
    syncMotion();
    waitingForGoto = false;
    clearTimeout(gotoTimer);
    if (persist) preferenceStatus(saved ? "Saved in this browser." : "Applied for this session; browser storage is unavailable.");
  }
  function preferencePayload() {
    return { version: 1, preferences: Object.assign({}, preferences.values) };
  }
  function parsePreferences(raw) {
    if (typeof raw !== "string" || raw.length > 16384) throw new Error("Preferences file is too large");
    return preferences.validate(JSON.parse(raw));
  }
  function preferenceLink() {
    var encoded = btoa(JSON.stringify(preferencePayload())).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    var url = new URL("preferences/", BASE);
    url.hash = "aggr-state=" + encoded;
    return url.href;
  }
  function renderPreferenceImport() {
    var panel = $("#preferences-import");
    if (!panel) return;
    panel.hidden = !pendingPreferences;
    var list = $("#preferences-import-summary");
    if (list) {
      list.replaceChildren();
      Object.keys(pendingPreferences || {}).forEach(function (key) {
        var control = $('[data-preference="' + key + '"]');
        var label = control && $("strong", control.closest("label"));
        var option = control && $$('option', control).find(function (entry) { return entry.value === pendingPreferences[key]; });
        var value = typeof pendingPreferences[key] === "boolean" ? pendingPreferences[key] ? "On" : "Off" : option ? option.textContent : pendingPreferences[key];
        list.appendChild(el("li", { text: (label ? label.textContent : key) + ": " + value }));
      });
    }
    if (preferenceImportMessage) preferenceStatus(preferenceImportMessage);
  }
  function importState() {
    var url = new URL(location.href);
    var fragment = new URLSearchParams(url.hash.slice(1));
    var encoded = fragment.get("aggr-state") || url.searchParams.get("aggr-state");
    if (!encoded) return;
    url.searchParams.delete("aggr-state");
    if (fragment.has("aggr-state")) url.hash = "";
    history.replaceState(history.state, "", url);
    try {
      if (encoded.length > 22000 || !/^[A-Za-z0-9_-]+={0,2}$/.test(encoded)) throw new Error("Invalid preferences link");
      var base64 = encoded.replace(/-/g, "+").replace(/_/g, "/");
      while (base64.length % 4) base64 += "=";
      pendingPreferences = parsePreferences(atob(base64));
      preferenceImportMessage = "Review the imported settings before applying them.";
      if (KIND !== "preferences") {
        var destination = new URL("preferences/", BASE);
        destination.hash = "aggr-state=" + encoded;
        location.replace(destination.href);
      }
    } catch (error) {
      pendingPreferences = null;
      preferenceImportMessage = "This preferences link is invalid or unsupported. Nothing was changed.";
    }
  }
  function copyState() {
    var url = preferenceLink();
    function fallback() {
      var field = $("#preferences-link");
      if (field) {
        field.hidden = false;
        field.value = url;
        field.focus();
        field.select();
      }
      preferenceStatus("Copy the selected link. Clipboard access is unavailable.");
    }
    if (!navigator.clipboard) { fallback(); return; }
    navigator.clipboard.writeText(url).then(function () { preferenceStatus("Preferences link copied."); }).catch(fallback);
  }
  function wirePreferences() {
    var page = $(".preferences");
    if (!page || page.dataset.bound === "true") return;
    page.dataset.bound = "true";
    $$("[data-preference]", page).forEach(function (control) {
      control.addEventListener("change", function () {
        var next = {};
        next[control.dataset.preference] = control.type === "checkbox" ? control.checked : control.value;
        applyPreferences(next, true);
      });
    });
    $$("[data-preferences-action]", page).forEach(function (button) {
      var action = button.dataset.preferencesAction;
      if (action === "share") button.hidden = !navigator.share;
      button.addEventListener("click", function () {
        if (action === "copy") copyState();
        if (action === "share") navigator.share({ title: "aggr preferences", url: preferenceLink() }).catch(function (error) {
          if (error.name !== "AbortError") preferenceStatus("Sharing is unavailable. Use Copy link or Save file.");
        });
        if (action === "save") {
          var url = URL.createObjectURL(new Blob([JSON.stringify(preferencePayload(), null, 2) + "\n"], { type: "application/json" }));
          var link = el("a", { href: url, download: "aggr-preferences.json" });
          document.body.appendChild(link);
          link.click();
          link.remove();
          setTimeout(function () { URL.revokeObjectURL(url); }, 1000);
          preferenceStatus("Preferences file saved.");
        }
        if (action === "import") $("#preferences-file").click();
        if (action === "apply" && pendingPreferences) {
          applyPreferences(pendingPreferences, true);
          pendingPreferences = null;
          preferenceImportMessage = "";
          renderPreferenceImport();
        }
        if (action === "cancel") {
          pendingPreferences = null;
          preferenceImportMessage = "Import cancelled. Your preferences were not changed.";
          renderPreferenceImport();
        }
        if (action === "reset") {
          var defaults = {};
          Object.keys(preferences.schema).forEach(function (key) { defaults[key] = preferences.schema[key].initial; });
          applyPreferences(defaults, true);
          preferenceStatus("Default preferences restored.");
        }
      });
    });
    var file = $("#preferences-file");
    if (file) file.addEventListener("change", function () {
      var selected = file.files[0];
      file.value = "";
      if (!selected) return;
      var reading = selected.size <= 16384 ? selected.text() : Promise.reject(new Error("File too large"));
      reading.then(function (raw) {
        pendingPreferences = parsePreferences(raw);
        preferenceImportMessage = "Review the imported settings before applying them.";
      }).catch(function () {
        pendingPreferences = null;
        preferenceImportMessage = "This file is invalid or unsupported. Use an aggr preferences JSON file (up to 16 KB). Nothing was changed.";
      }).then(function () { renderPreferenceImport(); });
    });
    var help = $("#show-shortcuts");
    if (help) help.addEventListener("click", function (event) {
      event.preventDefault();
      shortcutHelp();
    });
    renderPreferenceImport();
  }

  function renderResult(entry, display, rank) {
    var page = new URL(entry.url, BASE).href;
    display = display || {};
    var original = display.original || page;
    var date = entry.meta.date || "";
    var meta = [date ? el("time", { "class": "dt-published", datetime: date, title: date, text: date.slice(0, 10) }) : null, el("a", { "class": "u-bookmark-of", href: original, target: "_blank", rel: "external noopener noreferrer via", text: "original" })];
    var discussions = (display.discussions || []).filter(function (discussion) { return discussion.found; });
    discussions.forEach(function (discussion) {
      var url = discussion.url.replace("{url}", encodeURIComponent(original)).replace("{title}", encodeURIComponent(entry.meta.title || ""));
      meta.push(el("a", {
        "class": "discussion",
        href: url, target: "_blank", rel: "noopener noreferrer",
        "aria-label": discussion.name + ", matching discussion found",
        text: discussion.name.toLowerCase().replace(/\s+/g, "")
      }));
    });
    var excerpt = el("div", { "class": "search-excerpt" });
    if (entry.excerpt) excerpt.innerHTML = entry.excerpt;
    else excerpt.textContent = display.excerpt || "";
    return el("li", { "class": "row h-entry", "data-url": page, "data-link": original }, [
      el("a", { "class": "u-uid", href: page, hidden: "" }),
      el("div", { "class": "cell" }, [
        el("div", { "class": "row-content" }, [
          el("div", { "class": "row-copy" }, [
            el("div", { "class": "row-heading" }, [
              el("span", { "class": "rank", "aria-hidden": "true", text: rank + "." }),
              el("a", { "class": "title p-name u-url", "data-row-open": "", href: page, text: entry.meta.title }),
              display.domain ? el("a", { "class": "domain", href: BASE + "sources/" + display.source_slug + "/", text: display.domain }) : null
            ]),
            excerpt,
            el("div", { "class": "meta" }, meta.filter(Boolean))
          ]),
          previewMedia(display.preview)
        ])
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
          baseUrl: new URL(BASE).pathname,
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
      }).catch(function (error) {
        pagefind = null;
        throw error;
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
    if (!pagefindData.has(key)) {
      pagefindData.set(key, result.data().catch(function (error) {
        pagefindData.delete(key);
        throw error;
      }));
    }
    return pagefindData.get(key);
  }

  var FEED_PAGE_PARAMETER = "feed-page";
  function positiveInteger(value, fallback) {
    var parsed = Number.parseInt(value, 10);
    return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
  }
  function feedPageUrl(target, slice) {
    var current = new URL(location.href);
    var url = new URL(target, document.baseURI);
    current.searchParams.forEach(function (value, key) {
      if (key !== FEED_PAGE_PARAMETER && !url.searchParams.has(key)) url.searchParams.set(key, value);
    });
    if (slice > 1) url.searchParams.set(FEED_PAGE_PARAMETER, String(slice));
    else url.searchParams.delete(FEED_PAGE_PARAMETER);
    return url.href;
  }
  function updatePagerLink(pager, selector, target, slice, visible) {
    var link = $(selector, pager);
    if (!link) return;
    link.hidden = !visible;
    if (visible) link.href = feedPageUrl(target, slice);
  }
  function applyFeedPagination() {
    if (KIND !== "river") return;
    var list = $(".rows");
    var pager = $("[data-feed-pager]");
    if (!list || !pager) return;

    var rows = $$(".row", list);
    var staticSize = positiveInteger(pager.dataset.staticPageSize, rows.length || 1);
    var staticPage = positiveInteger(pager.dataset.staticPage, 1);
    var staticPages = positiveInteger(pager.dataset.staticPages, 1);
    var totalItems = Math.max(rows.length, positiveInteger(pager.dataset.totalItems, rows.length));
    var preferredSize = positiveInteger(preferences.values["feed-page-size"], 50);
    var pageSize = Math.min(preferredSize, staticSize);
    var slicesPerFullPage = Math.ceil(staticSize / pageSize);
    var slicesOnPage = Math.max(1, Math.ceil(rows.length / pageSize));
    var requestedSlice = positiveInteger(new URL(location.href).searchParams.get(FEED_PAGE_PARAMETER), 1);
    var slice = Math.min(requestedSlice, slicesOnPage);
    var start = (slice - 1) * pageSize;
    var end = Math.min(start + pageSize, rows.length);

    rows.forEach(function (row, index) {
      row.hidden = index < start || index >= end;
      if (row.hidden) row.classList.remove("is-selected");
    });
    list.dataset.feedPage = String(slice);
    list.dataset.feedPageSize = String(pageSize);

    var finalStaticCount = Math.max(0, totalItems - ((staticPages - 1) * staticSize));
    var finalSlices = Math.max(1, Math.ceil(finalStaticCount / pageSize));
    var totalPages = Math.max(1, ((staticPages - 1) * slicesPerFullPage) + finalSlices);
    var currentPage = ((staticPage - 1) * slicesPerFullPage) + slice;
    var hasPrevious = currentPage > 1;
    var hasNext = currentPage < totalPages;
    var previousTarget = slice > 1 ? location.href : pager.dataset.staticPrevious;
    var previousSlice = slice > 1 ? slice - 1 : slicesPerFullPage;
    var nextTarget = slice < slicesOnPage ? location.href : pager.dataset.staticNext;
    var nextSlice = slice < slicesOnPage ? slice + 1 : 1;
    var finalTarget = pager.dataset.staticLast || location.href;

    pager.hidden = totalPages <= 1;
    pager.dataset.page = String(currentPage);
    pager.dataset.pages = String(totalPages);
    var status = $("[data-page-status]", pager);
    if (status) status.textContent = "page " + currentPage + " / " + totalPages;
    updatePagerLink(pager, "[data-page-first]", pager.dataset.staticFirst || location.href, 1, hasPrevious);
    updatePagerLink(pager, "[data-page-previous]", previousTarget || location.href, previousSlice, hasPrevious);
    updatePagerLink(pager, "[data-page-next]", nextTarget || location.href, nextSlice, hasNext);
    updatePagerLink(pager, "[data-page-last]", finalTarget, finalSlices, hasNext);

    var normalized = feedPageUrl(location.href, pageSize < staticSize ? slice : 1);
    if (normalized !== location.href) history.replaceState(history.state, "", normalized);
    currentPageUrl = normalized;
  }
  function listRows() { return $$('.rows:not([aria-busy="true"]) .row:not([hidden])'); }
  function rowLink(row) { return row && $("[data-row-open]", row); }
  function listStateKey() {
    var url = new URL(currentPageUrl);
    url.hash = "";
    return entryStateKey("list-cursor") + ":" + encodeURIComponent(url.href);
  }
  function listState() {
    try { return JSON.parse(readSessionValue(listStateKey()) || "null") || {}; } catch (error) { return {}; }
  }
  function selectRow(row, focus, scroll) {
    var link = rowLink(row);
    if (!link) return false;
    listRows().forEach(function (entry) { entry.classList.toggle("is-selected", entry === row); });
    var state = listState();
    state.url = link.href;
    state.y = window.scrollY;
    writeSessionValue(listStateKey(), JSON.stringify(state));
    if (focus) link.focus({ preventScroll: true });
    if (scroll) row.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "instant" });
    return true;
  }
  function saveListPosition() {
    if (!listRows().length) return;
    var state = listState();
    var selected = rowLink($(".rows .row.is-selected"));
    if (selected) state.url = selected.href;
    state.y = window.scrollY;
    writeSessionValue(listStateKey(), JSON.stringify(state));
  }
  function restoreListCursor(focus, preferred) {
    var state = listState();
    var wanted = preferred || state.url;
    var row = listRows().find(function (entry) { var link = rowLink(entry); return link && link.href === wanted; });
    if (!row) return false;
    listRows().forEach(function (entry) { entry.classList.toggle("is-selected", entry === row); });
    if (focus) rowLink(row).focus({ preventScroll: true });
    if (focus && !preferred && typeof state.y === "number") window.scrollTo(0, state.y);
    return true;
  }
  function moveListCursor(direction) {
    var rows = listRows();
    if (!rows.length) return false;
    var current = rows.indexOf($(".rows .row.is-selected"));
    var next = current === -1 ? 0 : Math.max(0, Math.min(rows.length - 1, current + direction));
    return selectRow(rows[next], true, true);
  }
  function openSelectedResult() {
    var row = $(".rows .row.is-selected");
    var link = rowLink(row);
    if (!link) return false;
    selectRow(row, false, false);
    navigate(link.href);
    return true;
  }
  document.addEventListener("click", function (event) {
    var target = event.target instanceof Element && event.target.closest("[data-row-open]");
    if (target) selectRow(target.closest(".row"), false, false);
    if (event.target instanceof Element && event.target.closest("a[href]")) saveListPosition();
  }, true);
  window.addEventListener("pagehide", saveListPosition);

  function fillSearch() {
    var list = $("#list");
    var input = $("#q");
    if (!list || !input || input.dataset.bound === "true") return;
    input.dataset.bound = "true";
    var epoch = pageEpoch;
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
    function active(current) { return epoch === pageEpoch && input.isConnected && list.isConnected && (current === undefined || current === generation); }
    function show(rows, total, searched) {
      if (!active()) return;
      var focused = document.activeElement;
      var focusedUrl = focused && focused.matches("[data-row-open]") && list.contains(focused) ? focused.href : null;
      var fragment = document.createDocumentFragment();
      rows.forEach(function (entry, i) { fragment.appendChild(renderResult(entry.data, entry.display, i + 1)); });
      list.replaceChildren(fragment);
      enhancePreviewMedia(list);
      externalLinks(list);
      list.setAttribute("aria-busy", "false");
      empty.hidden = rows.length > 0;
      empty.textContent = searched ? "No matching items." : "Type to search the most recent items.";
      var label = total + (total === 1 ? " result" : " results");
      count.textContent = total > 999 ? "999+" : String(total);
      count.style.visibility = searched ? "visible" : "hidden";
      if (status) status.textContent = searched ? label : "";
      formatTimes(list);
      if ((focusedUrl || restoreListFocus) && restoreListCursor(true, focusedUrl)) restoreListFocus = false;
    }
    async function render() {
      var current = ++generation;
      var query = input.value.trim();
      var filters = {};
      if (category && category.value) filters.category = category.value;
      if (tag.value) filters.tag = tag.value;
      var searched = !!query || !!Object.keys(filters).length;
      if (!searched) { show([], 0, false); return current; }
      list.setAttribute("aria-busy", "true");
      try {
        var api = await loadPagefind();
        var options = { filters: filters };
        if (sort.value === "newest") options.sort = { date: "desc" };
        var found = query && api.debouncedSearch
          ? await api.debouncedSearch(query, options, 25)
          : await api.search(query || null, options);
        if (!found || !active(current)) return;
        var results = found.results.slice(0, 40);
        var first = results.slice(0, 12);
        var rows = await Promise.all(first.map(async function (result) {
          var data = await resultData(result);
          return { data: data, display: searchDisplay(data) };
        }));
        if (!active(current)) return;
        show(rows, found.results.length, true);
        if (results.length > first.length) {
          Promise.all(results.slice(first.length).map(async function (result) {
            var data = await resultData(result);
            return { data: data, display: searchDisplay(data) };
          })).then(function (rest) {
            if (active(current)) show(rows.concat(rest), found.results.length, true);
          }).catch(function () { /* keep the first results usable when later snippets fail */ });
        }
        return current;
      } catch (error) {
        if (active(current)) {
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
      currentPageUrl = url.href;
    }
    function schedule() {
      restoreListFocus = false;
      updateLocation();
      try { sessionStorage.removeItem(listStateKey()); } catch (error) { /* private mode */ }
      render();
    }
    count.style.visibility = "hidden";
    input.addEventListener("focus", function () { loadPagefind().catch(function () { /* render reports errors */ }); }, { once: true });
    input.addEventListener("input", schedule);
    if (category) category.addEventListener("change", schedule);
    tag.addEventListener("change", schedule);
    sort.addEventListener("change", schedule);
    if (form) form.addEventListener("submit", async function (event) {
      event.preventDefault();
      var rendered = await render();
      if (rendered !== undefined && active(rendered) && form.contains(document.activeElement)) openSelectedResult();
    });
    if (document.activeElement === input) loadPagefind().catch(function () { /* render reports errors */ });
    if (input.value.trim() || (category && category.value) || tag.value) render();
  }

  function restoreLocalFilter(input) {
    var query = new URL(location.href).searchParams.get("q");
    if (query !== null) input.value = query;
  }
  function updateLocalFilterLocation(input) {
    var url = new URL(location.href);
    var query = input.value.trim();
    if (query) url.searchParams.set("q", query);
    else url.searchParams.delete("q");
    history.replaceState(history.state, "", url);
    currentPageUrl = url.href;
  }

  function fillDirectory() {
    var input = $("[data-directory-filter]");
    if (!input || input.dataset.bound === "true") return;
    input.dataset.bound = "true";
    restoreLocalFilter(input);
    var root = input.closest("[data-directory-root]") || document;
    var rows = $$('[data-directory-entry]', root);
    var sections = $$('[data-directory-section]', root);
    var empty = $("[data-directory-empty]", root);
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
      sections.forEach(function (section) {
        var sectionRows = $$('[data-directory-entry]', section);
        if (sectionRows.length) section.hidden = !sectionRows.some(function (row) { return !row.hidden; });
      });
      if (empty) empty.hidden = visible !== 0;
    }
    input.addEventListener("input", function () {
      updateLocalFilterLocation(input);
      filter();
    });
    if (form) form.addEventListener("submit", function (event) {
      event.preventDefault();
      openFirstResult();
    });
    filter();
  }

  function fillListFilter() {
    var input = $("[data-list-filter]");
    if (!input || input.dataset.bound === "true") return;
    input.dataset.bound = "true";
    restoreLocalFilter(input);
    var rows = listRows();
    var count = $("#collection-count");
    var status = $("#collection-status");
    var empty = $("[data-list-empty]");
    var form = input.closest("form");
    function filter() {
      var tokens = input.value.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
      var visible = 0;
      rows.forEach(function (row) {
        var haystack = row.textContent.toLocaleLowerCase();
        var match = tokens.every(function (token) { return haystack.indexOf(token) !== -1; });
        row.hidden = !match;
        row.classList.remove("is-selected");
        if (match) visible += 1;
      });
      if (count) count.textContent = String(visible);
      if (status) status.textContent = visible + (visible === 1 ? " article" : " articles");
      if (empty) empty.hidden = visible !== 0;
    }
    input.addEventListener("input", function () {
      updateLocalFilterLocation(input);
      filter();
    });
    if (form) form.addEventListener("submit", function (event) {
      event.preventDefault();
      openSelectedResult();
    });
    filter();
  }

  function navigate(target) {
    var destination = new URL(target, document.baseURI).href;
    saveListPosition();
    if (updateState === "ready") location.assign(destination);
    else if (window.swup && typeof window.swup.navigate === "function") window.swup.navigate(destination);
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
    $$(".nav a[data-route]:not([target]), .mobile-nav a[data-route]").forEach(function (link) {
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
    if (updateState !== "ready" || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    var link = event.target instanceof Element && event.target.closest("a[href]");
    if (!link || link.hasAttribute("download") || link.target) return;
    var url = new URL(link.href);
    if (url.origin !== location.origin || url.pathname.indexOf(new URL(BASE).pathname) !== 0) return;
    if (url.pathname === location.pathname && url.search === location.search && url.hash) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    navigate(url.href);
  }, true);
  function shortcutHelp() {
    var dialog = $("#shortcut-help");
    if (!dialog) return;
    if (dialog.open) dialog.close();
    else {
      if (dialog.showModal) dialog.showModal();
      else dialog.setAttribute("open", "");
      var title = $("#shortcut-help-title", dialog);
      if (title) title.focus({ preventScroll: true });
    }
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
    var routes = { f: "", i: "", "/": "search/", l: "library/", p: "preferences/" };
    if (Object.prototype.hasOwnProperty.call(routes, key)) {
      if (key === "/") searchFocusRequested = true;
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
    return target instanceof Element && !!target.closest('input, textarea, select, button, [role="textbox"], [role="combobox"], [contenteditable]:not([contenteditable="false"])');
  }
  function articleNavigationDirection(key) {
    var normalized = key.length === 1 ? key.toLowerCase() : key;
    if (normalized === "k") return "previous";
    if (normalized === "j") return "next";
    return null;
  }
  function articleScroll(event) {
    if (KIND !== "item" || event.altKey || event.metaKey || event.ctrlKey || isEditing(event.target)) return false;
    var key = event.key.toLowerCase();
    if (key !== "d" && key !== "u") return false;
    var distance = Math.max(180, Math.min(420, window.innerHeight * 0.42));
    window.scrollBy(0, key === "d" ? distance : -distance);
    return true;
  }
  function openArticleExternal(url) {
    if (isInstalled()) location.assign(url);
    else window.open(url, "_blank", "noopener,noreferrer");
  }
  function articleExternalShortcut(key) {
    if (KIND !== "item" || key.length !== 1 || key !== key.toUpperCase()) return false;
    var article = $("article.item");
    if (!article) return false;
    if (key === "O") {
      var original = $(".u-bookmark-of", article);
      if (!original) return false;
      openArticleExternal(original.href);
      return true;
    }
    var networks = (window.AGGR && window.AGGR.discussions) || [];
    var network = networks.find(function (candidate) { return candidate.shortcut === key; });
    if (!network) return false;
    var found = $$(".discussion[data-discussion]", article).find(function (link) {
      return link.dataset.discussion === network.name;
    });
    var originalUrl = article.dataset.link || "";
    var title = $(".p-name", article);
    var target = found ? found.href : network.url
      .split("{url}").join(encodeURIComponent(originalUrl))
      .split("{title}").join(encodeURIComponent(title ? title.textContent : ""));
    openArticleExternal(target);
    return true;
  }
  document.addEventListener("keydown", function (event) {
    if (event.defaultPrevented || event.isComposing || event.keyCode === 229 || (event.getModifierState && event.getModifierState("AltGraph")) || $("dialog[open]")) return;
    if (!event.altKey && !event.shiftKey && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      if (KIND === "search" && focusPageSearch(true)) {
        return;
      } else {
        searchFocusRequested = true;
        navigate(new URL("search/", BASE).href);
      }
      return;
    }
    if (event.altKey || event.ctrlKey || event.metaKey || isEditing(event.target)) return;
    if (event.key.length === 1 && !singleKeyShortcuts()) return;
    if (event.key === "/" && !waitingForGoto) {
      event.preventDefault();
      if (KIND === "river" || !focusPageSearch(false)) {
        searchFocusRequested = true;
        navigate(new URL("search/", BASE).href);
      }
      return;
    }
    var focusedLink = event.target instanceof Element && event.target.closest("a[href]");
    if (event.key === "Enter" && focusedLink) return;
    if (articleScroll(event)) {
      event.preventDefault();
      return;
    }
    if (event.key === "?") {
      event.preventDefault();
      shortcutHelp();
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
    var direction = KIND === "item" && articleNavigationDirection(event.key);
    if (direction) {
      var article = $("article.item");
      var target = article && article.dataset[direction === "next" ? "nextUrl" : "previousUrl"];
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
          if (position.shortcutHelp && dialog && !dialog.open) {
            if (dialog.showModal) dialog.showModal();
            else dialog.setAttribute("open", "");
            var title = $("#shortcut-help-title", dialog);
            if (title) title.focus({ preventScroll: true });
          }
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
    else if (updateState === "ready") showConnectionStatus("Update ready for your next page.", false, false);
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
      var deltaX = Math.abs(event.touches[0].clientX - pullStartX);
      var deltaY = event.touches[0].clientY - pullStartY;
      if (deltaY <= 0 || deltaX > deltaY) {
        settlePullRefresh();
        return;
      }
      if (deltaY < 6) return;
      if (event.cancelable) event.preventDefault();
      var distance = Math.min(PULL_MAX, Math.round(deltaY * 0.55));
      if (deltaY >= PULL_THRESHOLD) setPullRefreshState("armed", distance);
      else setPullRefreshState("pulling", distance);
    }, { passive: false });

    document.addEventListener("touchend", function () {
      if (pullRefreshState === "armed") {
        clearTimeout(pullResetTimer);
        setPullRefreshState("refreshing", PULL_HOLD);
        showConnectionStatus("Refreshing for new items…", false, false);
        rememberReloadPosition();
        setTimeout(function () { location.reload(); }, 140);
      } else {
        settlePullRefresh();
      }
    }, { passive: true });
    document.addEventListener("touchcancel", settlePullRefresh, { passive: true });
  }

  function wirePersistentControls() {
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
    var controller = navigator.serviceWorker.controller;
    navigator.serviceWorker.addEventListener("controllerchange", function () {
      var nextController = navigator.serviceWorker.controller;
      if (controller && nextController && controller !== nextController) {
        updateState = "ready";
        document.documentElement.dataset.updateState = updateState;
        if (window.swup && window.swup.cache) window.swup.cache.clear();
        updateConnectionStatus();
      }
      controller = nextController;
    });
    navigator.serviceWorker.register(BASE + "sw.js", { updateViaCache: "none" }).then(function (registration) {
      var lastCheck = Date.now();
      function checkForUpdate() {
        if (!navigator.onLine || document.visibilityState !== "visible") return;
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
    currentPageUrl = location.href;
    var page = $("#aggr-page");
    if (page) {
      BASE = resolveRoot(page.dataset.root);
      KIND = page.dataset.kind;
      $("#aggr-base").setAttribute("href", BASE);
      document.body.dataset.kind = KIND;
      $$(".nav [data-route], .mobile-nav [data-route]").forEach(function (link) {
        link.setAttribute("href", new URL(link.dataset.route, document.baseURI).pathname);
      });
      $$(".nav [data-kinds], .mobile-nav [data-kinds]").forEach(function (link) {
        var active = link.dataset.kinds.split(/\s+/).indexOf(KIND) !== -1;
        if (active) link.setAttribute("aria-current", "page");
        else link.removeAttribute("aria-current");
      });
    }
    wireMenuNavigation();
    wireShortcutHelp();
    wirePersistentControls();
    wireTouchPullRefresh();
    importState();
    wirePreferences();
    applyPreferences(preferences.values, false);
    enhanceMarginNotes($("#swup") || document);
    enhancePreviewMedia($("#swup") || document);
    enhanceArticleMedia($("#swup") || document);
    externalLinks(document);
    fillSearch();
    fillDirectory();
    fillListFilter();
    if (KIND !== "search" && restoreListFocus && !window.swup) restoreListCursor(true);
    showNewEntries($("#swup") || document);
  }

  window.addEventListener("appinstalled", function () {
    externalLinks(document);
    wireTouchPullRefresh();
  });
  window.addEventListener("offline", updateConnectionStatus);
  window.addEventListener("online", updateConnectionStatus);

  if (window.performance && performance.getEntriesByType) {
    var navigationEntry = performance.getEntriesByType("navigation")[0];
    restoreListFocus = !!(navigationEntry && navigationEntry.type === "back_forward");
  }
  detectNewEntries();
  bootPage();
  restoreReloadPosition();
  var mobileNav = $(".mobile-nav");
  if (mobileNav && window.ResizeObserver) {
    new ResizeObserver(function () {
      var height = mobileNav.getBoundingClientRect().height;
      if (height) document.documentElement.style.setProperty("--bottom-nav-offset", height + "px");
      else document.documentElement.style.removeProperty("--bottom-nav-offset");
    }).observe(mobileNav);
  }
  var topBar = $(".top");
  if (topBar && window.ResizeObserver) {
    var syncTopNavOffset = function () {
      document.documentElement.style.setProperty("--top-nav-offset", topBar.getBoundingClientRect().height + "px");
    };
    new ResizeObserver(syncTopNavOffset).observe(topBar);
    syncTopNavOffset();
  }
  document.addEventListener("visibilitychange", function () {
    if (document.visibilityState === "visible") showNewEntries($("#swup") || document);
  });
  $$(".nav [data-route='search/'], .mobile-nav [data-route='search/']").forEach(function (searchLink) {
    ["pointerenter", "focus", "touchstart"].forEach(function (eventName) {
      searchLink.addEventListener(eventName, function () {
        loadPagefind().catch(function () { /* the search page reports an unavailable index */ });
      }, { once: true, passive: true });
    });
  });
  if (window.Swup) {
    window.swup = new window.Swup({ containers: ["#swup"], cache: true, animationSelector: false, native: nativeMotionEnabled(), animateHistoryBrowsing: false });
    window.swup.hooks.on("visit:start", function (visit) {
      saveListPosition();
      pageEpoch += 1;
      restoreListFocus = !!(visit && visit.history && visit.history.popstate);
    });
    window.swup.hooks.before("content:replace", function (visit) {
      syncPageHead(visit && visit.to && visit.to.document);
    });
    window.swup.hooks.on("page:view", function () {
      bootPage();
      announceNavigation();
      var restored = restoreListFocus && restoreListCursor(true);
      var target = searchFocusRequested && KIND === "search" ? $("#q") : restored ? null : $("#swup");
      searchFocusRequested = false;
      if (KIND !== "search") restoreListFocus = false;
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
  if (motionPreference) {
    if (motionPreference.addEventListener) motionPreference.addEventListener("change", syncMotion);
    else if (motionPreference.addListener) motionPreference.addListener(syncMotion);
  }
  if (darkPreference) {
    var syncAutoTheme = function () {
      if (document.documentElement.dataset.theme === "auto") applyPreferences(preferences.values, false);
    };
    if (darkPreference.addEventListener) darkPreference.addEventListener("change", syncAutoTheme);
    else if (darkPreference.addListener) darkPreference.addListener(syncAutoTheme);
  }
  window.addEventListener("storage", function (event) {
    if (event.key === null || Object.keys(preferences.schema).some(function (key) { return event.key === "aggr:" + key; })) {
      applyPreferences(preferences.read(), false);
      showNewEntries($("#swup") || document);
    }
  });
  serviceWorker();
  setInterval(function () { formatTimes($("#swup") || document); }, 60 * 1000);
})();
