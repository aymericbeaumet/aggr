// aggr default theme. No build step, no dependencies.
(function () {
  "use strict";
  var script = document.querySelector("script[src$='assets/app.js']");
  var BASE = (window.AGGR && window.AGGR.base) || (script && script.getAttribute("src").slice(0, -"assets/app.js".length)) || "/";
  var KIND = (window.AGGR && window.AGGR.kind) || document.body.getAttribute("data-kind") || "";
  var PWA = window.AGGR ? window.AGGR.pwa !== false : true;

  function $(selector, root) { return (root || document).querySelector(selector); }
  function $$(selector, root) { return Array.prototype.slice.call((root || document).querySelectorAll(selector)); }
  function el(tag, attrs, children) {
    var node = document.createElement(tag);
    Object.keys(attrs || {}).forEach(function (key) {
      if (key === "text") node.textContent = attrs[key]; else node.setAttribute(key, attrs[key]);
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
      var text = ago(time.getAttribute("datetime"));
      if (text) { time.title = time.textContent; time.textContent = text; }
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

  function renderRow(entry, rank) {
    var meta = [el("time", { datetime: entry.date, text: entry.date.slice(0, 10) }), document.createTextNode(" · "), el("a", { href: BASE + entry.url, text: "read" })];
    (entry.discussions || []).forEach(function (discussion) {
      meta.push(document.createTextNode(" · "));
      meta.push(el("a", { href: discussion.url, rel: "noopener noreferrer", text: discussion.name }));
    });
    return el("li", { "class": "row", "data-url": BASE + entry.url, "data-link": entry.link }, [
      el("span", { "class": "rank", text: rank + "." }),
      el("div", { "class": "cell" }, [
        el("a", { "class": "title", href: entry.link, rel: "noopener noreferrer", text: entry.title }),
        entry.domain ? el("a", { "class": "domain", href: BASE + "sources/" + entry.source + "/", text: "(" + entry.domain + ")" }) : null,
        el("div", { "class": "meta" }, meta)
      ])
    ]);
  }

  var index;
  function normalize(value) { return (value || "").toLocaleLowerCase().normalize("NFKD"); }
  function terms(query) { return normalize(query).split(/\s+/).filter(Boolean); }
  function fillSearch() {
    var list = $("#list");
    var input = $("#q");
    if (!list || !input) return;
    function render() {
      var query = terms(input.value);
      var rows = query.length ? index.filter(function (entry) {
        return query.every(function (term) { return entry.search.indexOf(term) !== -1; });
      }).slice(0, 500) : [];
      list.textContent = "";
      rows.forEach(function (entry, i) { list.appendChild(renderRow(entry, i + 1)); });
      $("#empty").hidden = rows.length > 0;
      $("#count").textContent = rows.length ? rows.length + (rows.length === 500 ? "+" : "") : "";
      relativeTimes(list);
    }
    fetch(BASE + "search.json", { credentials: "same-origin" })
      .then(function (response) { return response.json(); })
      .then(function (entries) { index = entries; render(); })
      .catch(function () { index = []; render(); });
    input.addEventListener("input", render);
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
    window.swup = new window.Swup({ containers: ["#swup"], cache: true });
    window.swup.hooks.on("page:view", bootPage);
  }
  serviceWorker();
})();
