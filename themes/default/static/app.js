// aggr default theme. No build step, no dependencies.
(function () {
  "use strict";
  var script = document.querySelector("script[src$='assets/app.js']");
  var BASE = (window.AGGR && window.AGGR.base) || (script && script.getAttribute("src").slice(0, -"assets/app.js".length)) || "./";
  var KIND = (window.AGGR && window.AGGR.kind) || document.body.getAttribute("data-kind") || "";
  var PWA = window.AGGR ? window.AGGR.pwa !== false : true;

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
  function relativeTimes(root) {
    $$("time[datetime]", root).forEach(function (time) {
      var exact = time.getAttribute("datetime");
      var text = ago(exact);
      if (text) { time.title = exact; time.textContent = text; }
    });
  }

  function setTheme(mode) {
    document.documentElement.dataset.theme = mode;
    try { localStorage.setItem("aggr:theme", mode); } catch (error) { /* private mode */ }
    var picker = $("#theme-mode");
    if (picker) picker.value = mode;
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
    navigator.clipboard.writeText(url.toString()).then(function () {
      var label = copy.textContent;
      copy.textContent = "copied";
      setTimeout(function () { copy.textContent = label; }, 1200);
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
    var category = $("#category-filter");
    var tag = $("#tag-filter");
    var sort = $("#search-sort");
    var generation = 0;
    function show(rows) {
      list.textContent = "";
      rows.forEach(function (entry, i) { list.appendChild(renderResult(entry, i + 1)); });
      $("#empty").hidden = rows.length > 0;
      $("#count").textContent = rows.length ? rows.length + (rows.length === 100 ? "+" : "") : "";
      relativeTimes(list);
    }
    async function render() {
      var current = ++generation;
      var query = input.value.trim();
      var filters = {};
      if (category.value) filters.category = category.value;
      if (tag.value) filters.tag = tag.value;
      if (!query && !Object.keys(filters).length) { show([]); return; }
      var api = await loadPagefind();
      var options = { filters: filters };
      if (sort.value === "newest") options.sort = { date: "desc" };
      var found = await api.search(query || null, options);
      var rows = await Promise.all(found.results.slice(0, 100).map(function (result) { return result.data(); }));
      if (current === generation) show(rows);
    }
    function addOptions(select, values) {
      Object.keys(values || {}).sort().forEach(function (value) {
        select.appendChild(el("option", { value: value, text: value + " (" + values[value] + ")" }));
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
    else if (event.key === "Enter" && row) location.href = row.dataset.url;
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

  var copy;
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
    copy = $("#copy-state");
    if (copy) copy.addEventListener("click", copyState);
    themeMode();
    relativeTimes($("#swup") || document);
    fillSearch();
  }

  importState();
  bootPage();
  if (window.Swup) {
    window.swup = new window.Swup({ containers: ["#swup"], cache: true, animationSelector: false });
    window.swup.hooks.on("history:popstate", function () { selected = -1; });
    window.swup.hooks.on("page:view", bootPage);
  }
  serviceWorker();
})();
