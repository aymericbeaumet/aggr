// aggr default theme. No build step, no dependencies. State lives in localStorage only.
(function () {
  "use strict";
  // The html view's CSP blocks the inline `window.AGGR`; fall back to the DOM.
  var script = document.querySelector("script[src$='assets/app.js']");
  var BASE = (window.AGGR && window.AGGR.base) || (script && script.getAttribute("src").slice(0, -"assets/app.js".length)) || "/";
  var KIND = (window.AGGR && window.AGGR.kind) || document.body.getAttribute("data-kind") || "";
  var PWA = window.AGGR ? window.AGGR.pwa !== false : true;
  var KEY = "aggr:v1";

  // --- state ---------------------------------------------------------------
  function load() {
    try {
      var raw = localStorage.getItem(KEY);
      var s = raw ? JSON.parse(raw) : {};
      return { read: s.read || {}, starred: s.starred || {} };
    } catch (e) {
      return { read: {}, starred: {} };
    }
  }
  var state = load();
  function save() {
    try { localStorage.setItem(KEY, JSON.stringify(state)); } catch (e) { /* private mode */ }
  }
  function isRead(path) { return Object.prototype.hasOwnProperty.call(state.read, path); }
  function isStarred(path) { return Object.prototype.hasOwnProperty.call(state.starred, path); }
  function markRead(path, on) {
    if (on === false) delete state.read[path]; else state.read[path] = Date.now();
    save();
  }
  function toggleStar(path) {
    if (isStarred(path)) delete state.starred[path]; else state.starred[path] = Date.now();
    save();
    return isStarred(path);
  }

  // --- helpers -------------------------------------------------------------
  function $(sel, root) { return (root || document).querySelector(sel); }
  function $$(sel, root) { return Array.prototype.slice.call((root || document).querySelectorAll(sel)); }
  function el(tag, attrs, children) {
    var node = document.createElement(tag);
    Object.keys(attrs || {}).forEach(function (k) {
      if (k === "text") node.textContent = attrs[k];
      else if (k === "html") node.innerHTML = attrs[k];
      else node.setAttribute(k, attrs[k]);
    });
    (children || []).forEach(function (c) { if (c) node.appendChild(c); });
    return node;
  }
  function ago(iso) {
    var t = Date.parse(iso);
    if (isNaN(t)) return null;
    var s = Math.round((Date.now() - t) / 1000);
    if (s < 0) s = 0;
    if (s < 60) return "just now";
    var m = Math.round(s / 60);
    if (m < 60) return m + "m ago";
    var h = Math.round(m / 60);
    if (h < 36) return h + "h ago";
    var d = Math.round(h / 24);
    if (d < 45) return d + "d ago";
    var mo = Math.round(d / 30);
    if (mo < 18) return mo + "mo ago";
    return Math.round(d / 365) + "y ago";
  }
  function relativeTimes(root) {
    $$("time[datetime]", root).forEach(function (t) {
      var text = ago(t.getAttribute("datetime"));
      if (text) { t.setAttribute("title", t.textContent); t.textContent = text; }
    });
  }

  // --- rows ----------------------------------------------------------------
  function decorate(row) {
    var path = row.getAttribute("data-path");
    row.classList.toggle("is-read", isRead(path));
    row.classList.toggle("is-starred", isStarred(path));
    var star = $(".star", row);
    if (star) star.textContent = isStarred(path) ? "★" : "☆";
  }
  function decorateAll() { $$(".row, .item").forEach(decorate); }

  // Mirrors _item.html; used by the client-side shells.
  function renderRow(entry, rank) {
    var url = BASE + entry.url;
    var meta = el("div", { "class": "meta" }, [
      el("a", { href: BASE + "sources/" + entry.source + "/", text: entry.source_name }),
      el("span", {}, [document.createTextNode("· "), el("time", { datetime: entry.date, text: entry.date.slice(0, 10) })]),
      el("span", {}, [document.createTextNode("· "), el("a", { "class": "read", href: url, text: "read" })])
    ]);
    var cell = el("div", { "class": "cell" }, [
      el("a", { "class": "title", href: entry.link, rel: "noopener noreferrer", text: entry.title }),
      entry.domain ? el("span", { "class": "domain", text: "(" + entry.domain + ")" }) : null,
      meta
    ]);
    return el("li", { "class": "row", "data-path": entry.path, "data-url": url, "data-link": entry.link }, [
      el("span", { "class": "rank", text: rank + "." }),
      el("button", { "class": "star", type: "button", "aria-label": "Star", title: "Star (s)", text: "☆" }),
      cell
    ]);
  }

  // --- shells (unread / starred / search) ------------------------------------
  var index = null;
  function loadIndex(cb) {
    if (index) return cb(index);
    fetch(BASE + "search.json", { credentials: "same-origin" })
      .then(function (r) { return r.json(); })
      .then(function (data) { index = data; cb(index); })
      .catch(function () { cb([]); });
  }
  function tokens(q) { return q.toLowerCase().split(/\s+/).filter(Boolean); }
  function matches(entry, terms) {
    var hay = (entry.title + " " + entry.source_name + " " + entry.domain + " " + (entry.category || "") + " " + entry.excerpt).toLowerCase();
    return terms.every(function (t) { return hay.indexOf(t) !== -1; });
  }
  // Site paths the service worker holds, across the precache and the runtime cache.
  function cachedPaths(cb) {
    if (!("caches" in window)) return cb({});
    var paths = {};
    caches.keys().then(function (names) {
      return Promise.all(names.map(function (name) {
        return caches.open(name).then(function (c) { return c.keys(); }).then(function (reqs) {
          reqs.forEach(function (r) { paths[new URL(r.url).pathname] = true; });
        });
      }));
    }).then(function () { cb(paths); }, function () { cb(paths); });
  }
  function fillShell() {
    var list = $("#list");
    if (!list) return;
    var kind = list.getAttribute("data-shell");
    var q = $("#q");
    function show(rows) {
      list.textContent = "";
      rows.slice(0, 500).forEach(function (e, i) { list.appendChild(renderRow(e, i + 1)); });
      var empty = $("#empty");
      if (empty) empty.hidden = rows.length > 0;
      var count = $("#count");
      if (count) count.textContent = rows.length ? rows.length + (rows.length === 500 ? "+" : "") : "";
      decorateAll();
      relativeTimes(list);
    }
    function render() {
      loadIndex(function (entries) {
        if (kind === "unread") show(entries.filter(function (e) { return !isRead(e.path); }));
        else if (kind === "starred") show(entries.filter(function (e) { return isStarred(e.path); }));
        else if (kind === "offline") cachedPaths(function (paths) { show(entries.filter(function (e) { return paths[BASE + e.url]; })); });
        else {
          var terms = tokens(q ? q.value : "");
          show(terms.length ? entries.filter(function (e) { return matches(e, terms); }) : []);
        }
      });
    }
    if (q) {
      var timer;
      q.addEventListener("input", function () { clearTimeout(timer); timer = setTimeout(render, 120); });
    }
    render();
  }

  // --- events ----------------------------------------------------------------
  document.addEventListener("click", function (ev) {
    var star = ev.target.closest(".star");
    if (star) {
      var holder = star.closest("[data-path]");
      if (holder) { toggleStar(holder.getAttribute("data-path")); decorate(holder); }
      ev.preventDefault();
      return;
    }
    var action = ev.target.closest("[data-action]");
    if (action && action.getAttribute("data-action") === "mark-all-read") {
      $$(".row").forEach(function (row) { markRead(row.getAttribute("data-path")); decorate(row); });
      if (KIND === "unread") fillShell();
      return;
    }
    var link = ev.target.closest("a.title, a.read");
    if (link) {
      var row = link.closest(".row");
      if (row) { markRead(row.getAttribute("data-path")); decorate(row); }
    }
  });
  document.addEventListener("change", function (ev) {
    var box = ev.target.closest("[data-action=hide-read]");
    if (box) {
      document.body.classList.toggle("hide-read", box.checked);
      try { localStorage.setItem(KEY + ":hide-read", box.checked ? "1" : ""); } catch (e) { /* ignore */ }
    }
  });

  // Keyboard: j/k move, o open original, enter open item page, s star, r toggle read.
  var selected = -1;
  function rows() { return $$(".row").filter(function (r) { return r.offsetParent !== null; }); }
  function select(i) {
    var all = rows();
    if (!all.length) return;
    selected = Math.max(0, Math.min(all.length - 1, i));
    all.forEach(function (r, n) { r.classList.toggle("is-selected", n === selected); });
    all[selected].scrollIntoView({ block: "nearest" });
  }
  document.addEventListener("keydown", function (ev) {
    if (ev.altKey || ev.ctrlKey || ev.metaKey) return;
    var tag = (ev.target.tagName || "").toLowerCase();
    if (tag === "input" || tag === "textarea" || ev.target.isContentEditable) return;
    var all = rows();
    var row = selected >= 0 ? all[selected] : null;
    switch (ev.key) {
      case "j": select(selected + 1); break;
      case "k": select(selected - 1); break;
      case "o":
        if (row) { markRead(row.getAttribute("data-path")); decorate(row); window.open(row.getAttribute("data-link"), "_blank", "noopener"); }
        break;
      case "Enter":
        if (row) { markRead(row.getAttribute("data-path")); location.href = row.getAttribute("data-url"); }
        break;
      case "s": {
        var holder = row || $(".item");
        if (holder) { toggleStar(holder.getAttribute("data-path")); decorate(holder); }
        break;
      }
      case "r":
        if (row) { var p = row.getAttribute("data-path"); markRead(p, !isRead(p)); decorate(row); }
        break;
      default: return;
    }
    ev.preventDefault();
  });

  // --- service worker ----------------------------------------------------------
  function toast(text, action, onAction) {
    var button = el("button", { type: "button", "class": "linkish", text: action });
    button.addEventListener("click", onAction);
    document.body.appendChild(el("div", { "class": "toast", role: "status" }, [document.createTextNode(text), button]));
  }
  function serviceWorker() {
    if (!("serviceWorker" in navigator) || location.protocol === "file:") return;
    if (!PWA) {
      // Turned off in the config: let a worker from an earlier build go.
      navigator.serviceWorker.getRegistrations().then(function (rs) { rs.forEach(function (r) { r.unregister(); }); });
      return;
    }
    if (KIND === "html") return;
    var hadController = !!navigator.serviceWorker.controller;
    navigator.serviceWorker.addEventListener("controllerchange", function () {
      if (hadController) toast("Updated — ", "reload", function () { location.reload(); });
      hadController = true;
    });
    navigator.serviceWorker.register(BASE + "sw.js").catch(function () { /* insecure context */ });
  }

  // --- boot ------------------------------------------------------------------
  var item = $(".item[data-path]");
  if (item) markRead(item.getAttribute("data-path"));
  try {
    if (localStorage.getItem(KEY + ":hide-read")) {
      document.body.classList.add("hide-read");
      var box = $("[data-action=hide-read]");
      if (box) box.checked = true;
    }
  } catch (e) { /* ignore */ }
  decorateAll();
  relativeTimes(document);
  fillShell();
  serviceWorker();
})();
