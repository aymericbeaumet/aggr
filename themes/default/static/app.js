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
    var minutes = Math.round(seconds / 60);
    if (minutes < 60) return minutes + "m ago";
    var hours = Math.round(minutes / 60);
    if (hours < 36) return hours + "h ago";
    var days = Math.round(hours / 24);
    if (days < 45) return days + "d ago";
    var months = Math.round(days / 30);
    if (months < 18) return months + "mo ago";
    return Math.round(days / 365) + "y ago";
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
      if (text) { time.title = exact; time.textContent = text; }
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

  function renderResult(entry, rank) {
    var page = BASE + entry.url.replace(/^\/+/, "");
    var original = entry.meta.original || page;
    var date = entry.meta.date || "";
    var meta = [date ? el("time", { datetime: date, title: date, text: date.slice(0, 10) }) : null, document.createTextNode(" · "), el("a", { href: original, target: "_blank", rel: "noopener noreferrer", text: "original" })];
    ((window.AGGR && window.AGGR.discussions) || []).forEach(function (discussion) {
      var url = discussion.url.replace("{url}", encodeURIComponent(original)).replace("{title}", encodeURIComponent(entry.meta.title || ""));
      meta.push(document.createTextNode(" · "));
      meta.push(el("a", { href: url, target: "_blank", rel: "noopener noreferrer", text: discussion.name.toLowerCase().replace(/\s+/g, "") }));
    });
    return el("li", { "class": "row", "data-url": page, "data-link": original }, [
      el("span", { "class": "rank", text: rank + "." }),
      el("div", { "class": "cell" }, [
        el("a", { "class": "title", href: page, text: entry.meta.title }),
        entry.meta.domain ? el("a", { "class": "domain", href: BASE + "sources/" + entry.meta.source_slug + "/", text: "(" + entry.meta.domain + ")" }) : null,
        el("div", { "class": "search-excerpt", html: entry.excerpt }),
        el("div", { "class": "meta" }, meta.filter(Boolean))
      ])
    ]);
  }

  var pagefind;
  function loadPagefind() {
    if (!pagefind) {
      pagefind = import(new URL("pagefind/pagefind.js", document.baseURI).href).then(async function (api) {
        await api.options({ baseUrl: new URL(".", document.baseURI).pathname });
        await api.init();
        return api;
      });
    }
    return pagefind;
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
    var empty = $("#empty");
    var generation = 0;
    function show(rows, searched) {
      list.textContent = "";
      rows.forEach(function (entry, i) { list.appendChild(renderResult(entry, i + 1)); });
      list.setAttribute("aria-busy", "false");
      empty.hidden = rows.length > 0;
      empty.textContent = searched ? "No matching items." : "Type to search the most recent items.";
      var label = rows.length === 100 ? "100 or more results" : rows.length + (rows.length === 1 ? " result" : " results");
      count.textContent = rows.length === 100 ? "100+" : String(rows.length);
      count.setAttribute("aria-label", label);
      formatTimes(list);
    }
    async function render() {
      var current = ++generation;
      var query = input.value.trim();
      var filters = {};
      if (category.value) filters.category = category.value;
      if (tag.value) filters.tag = tag.value;
      var searched = !!query || !!Object.keys(filters).length;
      if (!searched) { show([], false); return; }
      list.setAttribute("aria-busy", "true");
      try {
        var api = await loadPagefind();
        var options = { filters: filters };
        if (sort.value === "newest") options.sort = { date: "desc" };
        var found = await api.search(query || null, options);
        var rows = await Promise.all(found.results.slice(0, 100).map(function (result) { return result.data(); }));
        if (current === generation) show(rows, true);
      } catch (error) {
        if (current === generation) {
          show([], true);
          empty.textContent = "Search is unavailable right now.";
        }
      }
    }
    function addOptions(select, values) {
      Object.keys(values || {}).sort().forEach(function (value) {
        var total = Number(values[value]) || 0;
        if (total > 0) select.appendChild(el("option", { value: value, text: value + " (" + total + ")" }));
      });
    }
    loadPagefind().then(function (api) { return api.filters(); }).then(function (filters) {
      addOptions(category, filters.category);
      addOptions(tag, filters.tag);
    });
    input.addEventListener("input", render);
    category.addEventListener("change", render);
    tag.addEventListener("change", render);
    sort.addEventListener("change", render);
    if (form) form.addEventListener("submit", function (event) { event.preventDefault(); render(); });
  }

  var selected = -1;
  function rows() { return $$(".row"); }
  function select(index) {
    var all = rows();
    if (!all.length) return;
    selected = Math.max(0, Math.min(all.length - 1, index));
    all.forEach(function (row, i) { row.classList.toggle("is-selected", i === selected); });
    all[selected].scrollIntoView({ block: "nearest" });
  }
  document.addEventListener("keydown", function (event) {
    if (event.altKey || event.ctrlKey || event.metaKey || /^(input|textarea)$/i.test(event.target.tagName)) return;
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
      var main = $("#swup");
      if (main) main.focus({ preventScroll: true });
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
})();
