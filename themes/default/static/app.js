// aggr default theme. No build step, no dependencies.
(function () {
  "use strict";
  var script = document.querySelector("script[src$='assets/app.js']");
  var BASE = (window.AGGR && window.AGGR.base) || (script && script.getAttribute("src").slice(0, -"assets/app.js".length)) || "./";
  var KIND = (window.AGGR && window.AGGR.kind) || document.body.getAttribute("data-kind") || "";
  var PWA = window.AGGR ? window.AGGR.pwa !== false : true;
  var darkPreference = window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)");

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
    applyAgeBoundaries(root);
  }

  function ageBand(iso) {
    var age = Math.max(0, Date.now() - Date.parse(iso));
    if (age < 60 * 60 * 1000) return "fresh";
    if (age < 3 * 60 * 60 * 1000) return "h1";
    if (age < 24 * 60 * 60 * 1000) return "h3";
    return "h24";
  }
  function applyAgeBoundaries(root) {
    $$(".rows:not(.search-results)", root).forEach(function (list) {
      var previous = null;
      $$(".row", list).forEach(function (row) {
        var time = $(".meta time[datetime]", row);
        if (!time) return;
        var band = ageBand(time.getAttribute("datetime"));
        row.classList.remove("age-fresh", "age-h1", "age-h3", "age-h24");
        row.classList.add("age-" + band);
        row.dataset.age = band;
        row.classList.toggle("age-boundary", previous !== null && previous !== band);
        previous = band;
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

  function setTheme(mode) {
    document.documentElement.dataset.theme = mode;
    try { localStorage.setItem("aggr:theme", mode); } catch (error) { /* private mode */ }
    var picker = $("#theme-mode");
    if (picker) picker.value = mode;
    var dark = mode === "dark" || (mode === "auto" && darkPreference && darkPreference.matches);
    var meta = $("#theme-color");
    if (meta) meta.setAttribute("content", dark ? "#180f0e" : "#2b1a17");
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
    var meta = [date ? el("time", { datetime: date, title: date, text: date.slice(0, 10) }) : null, document.createTextNode(" · "), el("a", { href: original, target: "_blank", rel: "noopener noreferrer", text: "original" })];
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
    return el("li", { "class": "row", "data-url": page, "data-link": original }, [
      el("span", { "class": "rank", text: rank + "." }),
      el("div", { "class": "cell" }, [
        el("a", { "class": "title", href: page, text: entry.meta.title }),
        display.domain ? el("a", { "class": "domain", href: BASE + "sources/" + display.source_slug + "/", text: "(" + display.domain + ")" }) : null,
        excerpt,
        el("div", { "class": "meta" }, meta.filter(Boolean))
      ])
    ]);
  }

  var pagefind;
  var searchMetadata;
  function loadPagefind() {
    if (!pagefind) {
      pagefind = import(new URL("pagefind/pagefind.js", document.baseURI).href).then(async function (api) {
        await api.options({ baseUrl: new URL(".", document.baseURI).pathname, excerptLength: 24 });
        await api.init();
        return api;
      });
    }
    return pagefind;
  }
  function loadSearchMetadata() {
    if (!searchMetadata) {
      searchMetadata = fetch(new URL("search-meta.json", document.baseURI).href, { credentials: "same-origin" })
        .then(function (response) { if (!response.ok) throw new Error("search metadata unavailable"); return response.json(); });
    }
    return searchMetadata;
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
    var generation = 0;
    var timer;
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
        var loaded = await Promise.all([loadPagefind(), loadSearchMetadata()]);
        var api = loaded[0];
        var metadata = loaded[1];
        var options = { filters: filters };
        if (sort.value === "newest") options.sort = { date: "desc" };
        var found = await api.search(query || null, options);
        var rows = await Promise.all(found.results.slice(0, 40).map(async function (result) {
          var data = await result.data();
          return { data: data, display: metadata[data.url.replace(/^\/+/, "")] || {} };
        }));
        if (current === generation) show(rows, found.results.length, true);
      } catch (error) {
        if (current === generation) {
          show([], 0, true);
          empty.textContent = "Search is unavailable right now.";
        }
      }
    }
    function schedule() {
      clearTimeout(timer);
      timer = setTimeout(render, 80);
      var query = input.value.trim();
      if (query) {
        loadPagefind()
          .then(function (api) { return api.preload ? api.preload(query) : null; })
          .catch(function () { /* render reports an unavailable index without an unhandled rejection */ });
      }
    }
    count.style.visibility = "hidden";
    input.addEventListener("focus", loadPagefind, { once: true });
    input.addEventListener("input", schedule);
    category.addEventListener("change", render);
    tag.addEventListener("change", render);
    sort.addEventListener("change", render);
    if (form) form.addEventListener("submit", function (event) { event.preventDefault(); render(); });
    if (document.activeElement === input) loadPagefind();
  }

  var selected = -1;
  function rows() { return $$(".row"); }
  function select(index) {
    var all = rows();
    if (!all.length) return;
    selected = Math.max(0, Math.min(all.length - 1, index));
    all.forEach(function (row, i) { row.classList.toggle("is-selected", i === selected); });
    all[selected].scrollIntoView({ block: "nearest" });
    var title = $("a.title", all[selected]);
    if (title) title.focus({ preventScroll: true });
  }
  document.addEventListener("keydown", function (event) {
    if (event.altKey || event.ctrlKey || event.metaKey || event.target.closest("input, textarea, select, button, a, [contenteditable=true]")) return;
    var row = selected >= 0 ? rows()[selected] : null;
    if (event.key === "j") select(selected + 1);
    else if (event.key === "k") select(selected - 1);
    else if (event.key === "o" && row) window.open(row.dataset.link, "_blank", "noopener");
    else if (event.key === "Enter" && row) {
      if (window.swup) window.swup.navigate(row.dataset.url);
      else location.href = row.dataset.url;
    }
    else return;
    event.preventDefault();
  });

  function toast(text, action, onAction) {
    var button = el("button", { type: "button", "class": "linkish", text: action });
    button.addEventListener("click", onAction);
    document.body.appendChild(el("div", { "class": "toast", role: "status" }, [document.createTextNode(text), button]));
  }
  function serviceWorker() {
    if (!("serviceWorker" in navigator) || location.protocol === "file:") return;
    if (!PWA) {
      navigator.serviceWorker.getRegistrations().then(function (registrations) { registrations.forEach(function (registration) { registration.unregister(); }); });
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
      BASE = page.dataset.root;
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
  if (window.Swup) {
    window.swup = new window.Swup({ containers: ["#swup"], cache: true, animationSelector: false });
    window.swup.hooks.on("history:popstate", function () { selected = -1; });
    window.swup.hooks.on("page:view", function () {
      selected = -1;
      bootPage();
      var target = KIND === "search" ? $("#q") : $("#swup");
      if (target) target.focus({ preventScroll: true });
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
