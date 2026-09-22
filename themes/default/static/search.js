// @ts-check
/**
 * Full-text search, loaded the first time someone reaches for the search field.
 *
 * Pagefind builds and serves the index; everything here is the query language, the completion
 * menu and the result list on top of it. The chrome is already in the HTML: this file fills it.
 */

const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => Array.from(root.querySelectorAll(selector));

const FACET_FIELDS = ["source", "category", "tag", "type"];
const DATE_FIELDS = ["date", "before", "after", "since", "until"];
const OPERATORS = [
  "source:", "category:", "tag:", "type:", "date:", "after:", "before:", "since:", "until:", "sort:",
];
const DATE_SHORTCUTS = ["today", "yesterday", "last7d", "last30d", "year"];
const MAX_CHARS = 4096;
const MAX_CLAUSES = 16;
const DEBOUNCE = 180;

class QueryError extends Error {
  constructor(message, start = 0, end = start) {
    super(message);
    this.start = start;
    this.end = end;
  }
}

/* ------------------------------------------------------------------ query language */

/** Split a query into clauses, honouring quoted phrases and backslash escapes. */
function tokenize(raw, incomplete = false) {
  const tokens = [];
  let i = 0;
  while (i < raw.length) {
    if (/\s/u.test(raw[i])) {
      i++;
      continue;
    }
    const start = i;
    let value = "";
    let quote = false;
    let quoted = false;
    while (i < raw.length && (quote || !/\s/u.test(raw[i]))) {
      const char = raw[i++];
      if (char === "\\" && i < raw.length && /["\\]/.test(raw[i])) value += raw[i++];
      else if (char === '"') {
        quote = !quote;
        quoted = true;
      } else value += char;
    }
    if (quote && !incomplete)
      throw new QueryError("Close the quoted phrase with a double quote.", start, i);
    tokens.push({ raw: raw.slice(start, i), value, start, end: i, quoted });
  }
  return tokens;
}

const quoteValue = (value) =>
  /[\s"\\]/u.test(value) ? '"' + value.replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"' : value;

const DAY = 86400000;
const day = (timestamp) => new Date(timestamp).toISOString().slice(0, 10);

function checkedDay(value) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value) || !Number.isFinite(Date.parse(value)) || day(Date.parse(value)) !== value)
    throw new QueryError("Use a valid UTC date, such as 2026-09-08.");
  return value;
}
const shifted = (value, days) => day(Date.parse(value) + days * DAY);

/** Resolve a date qualifier to an inclusive range of UTC publication days. */
function dateRange(value, operator, now) {
  const today = day(now);
  if (operator === "date") {
    if (value === "today") return { from: today, through: today };
    if (value === "yesterday") return { from: shifted(today, -1), through: shifted(today, -1) };
    const shortcut = { week: 7, last7d: 7, month: 30, last30d: 30, year: 365 }[value];
    if (shortcut) return { from: shifted(today, 1 - shortcut), through: today };
    const range = value.split("..");
    if (range.length === 2) {
      const from = range[0] ? checkedDay(range[0]) : undefined;
      const through = range[1] ? checkedDay(range[1]) : undefined;
      if ((!from && !through) || (from && through && from > through))
        throw new QueryError("The date range must run from an earlier date to a later date.");
      return { from, through };
    }
    const comparison = value.match(/^(>=|<=|>|<|=)(.+)$/);
    if (comparison)
      return dateRange(
        comparison[2],
        { ">=": "since", "<=": "until", ">": "after", "<": "before", "=": "date" }[comparison[1]],
        now,
      );
  }
  const exact = checkedDay(value);
  if (operator === "after") return { from: shifted(exact, 1) };
  if (operator === "before") return { through: shifted(exact, -1) };
  if (operator === "since") return { from: exact };
  if (operator === "until") return { through: exact };
  return { from: exact, through: exact };
}

function parseQuery(raw, now = Date.now()) {
  if (raw.length > MAX_CHARS)
    throw new QueryError("Search is limited to 4,096 characters.", MAX_CHARS, raw.length);
  const tokens = tokenize(raw);
  if (tokens.length > MAX_CLAUSES)
    throw new QueryError("Use at most 16 search clauses.", tokens[MAX_CLAUSES].start, raw.length);
  let sort = "relevance";
  const clauses = tokens.map((token) => {
    const exclude = token.value.startsWith("-") && !token.raw.startsWith('"');
    const value = exclude ? token.value.slice(1) : token.value;
    if (!value.trim())
      throw new QueryError("Add a word or phrase to search for.", token.start, token.end);
    const clause = { ...token, value, exclude, kind: "text" };
    // A pasted URL is full text, not a qualifier, however many colons it holds.
    const operator =
      token.raw.startsWith('"') || token.raw.startsWith('-"')
        ? null
        : value.match(/^([a-z-]+):([\s\S]*)$/i);
    if (!operator || /^https?:\/\//i.test(value)) return clause;
    const field = operator[1].toLowerCase();
    const argument = operator[2];
    if (![...FACET_FIELDS, ...DATE_FIELDS, "sort"].includes(field)) return clause;
    if (!argument) throw new QueryError("Add a value after " + field + ":", token.start, token.end);
    if (FACET_FIELDS.includes(field)) return { ...clause, kind: "facet", field, value: argument };
    if (DATE_FIELDS.includes(field))
      return {
        ...clause,
        kind: "date",
        value: argument,
        ...dateRange(argument.toLowerCase(), field, now),
      };
    if (exclude || !["relevance", "newest", "oldest"].includes(argument))
      throw new QueryError("Sort by relevance, newest, or oldest.", token.start, token.end);
    sort = argument;
    return { ...clause, kind: "sort", value: argument };
  });
  return { raw, clauses, sort };
}

function dateMatches(clause, date) {
  const matches = (!clause.from || date >= clause.from) && (!clause.through || date <= clause.through);
  return clause.exclude ? !matches : matches;
}

/** Replace relative date shortcuts with absolute dates, so a shared link stays stable. */
function canonicalQuery(query, now = Date.now()) {
  let canonical = query;
  for (const clause of parseQuery(query, now).clauses.reverse()) {
    if (clause.kind !== "date" || !/^(today|yesterday|week|last7d|month|last30d|year)$/.test(clause.value))
      continue;
    const value =
      clause.from === clause.through ? clause.from : (clause.from || "") + ".." + (clause.through || "");
    canonical =
      canonical.slice(0, clause.start) +
      (clause.exclude ? "-" : "") +
      "date:" +
      value +
      canonical.slice(clause.end);
  }
  return canonical;
}

function queryURL(base, query, page = 1, now = Date.now()) {
  const url = new URL(base);
  if (query.trim()) url.searchParams.set("q", canonicalQuery(query, now));
  if (page > 1) url.searchParams.set("search-page", String(page));
  return url.href;
}

const facetURL = (base, field, value) =>
  queryURL(base, field + ':"' + value.replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"');

/** The collection page a facet already has, matching `facet_page` in the templates. */
const facetPage = (base, field, value) =>
  new URL((field === "source" ? "sources/" : field === "category" ? "categories/" : "tags/") + value + "/", base)
    .href;

/* ------------------------------------------------------------------ facets */

const normalized = (value) => value.normalize("NFKC").toLocaleLowerCase();

/**
 * Facet values by identifier and by readable label. A label shared by two values resolves to
 * nothing rather than silently picking one.
 */
function facetIndex(values, kind) {
  const identifiers = new Map();
  const labels = new Map();
  for (const facet of values) {
    if (!identifiers.has(facet.value)) identifiers.set(facet.value, facet);
    const label = normalized(facet.label);
    labels.set(label, labels.has(label) ? null : facet);
  }
  const alias = (facet) => {
    // A source is named by its hostname everywhere it is published, including in a query: that
    // name is already the readable one, and a display name would not survive being shared.
    if (kind === "source") return undefined;
    if (!facet.label.trim()) return undefined;
    const label = normalized(facet.label);
    if (label === normalized(facet.value)) return facet.value;
    if ((identifiers.get(facet.label) ?? labels.get(label))?.value === facet.value) return facet.label;
    return undefined;
  };
  const entries = values
    .map((facet) => ({
      facet,
      alias: alias(facet),
      search: normalized(facet.label + " " + facet.value),
    }))
    .sort((a, b) => b.facet.count - a.facet.count || a.facet.label.localeCompare(b.facet.label));
  return {
    entries,
    alias,
    lookup: (value) => identifiers.get(value) ?? labels.get(normalized(value)),
  };
}

const indexCache = new WeakMap();
function cachedIndex(values, kind) {
  let index = indexCache.get(values);
  if (!index) indexCache.set(values, (index = facetIndex(values, kind)));
  return index;
}

function resolveFacet(value, values, kind) {
  const facet = cachedIndex(values, kind).lookup(value);
  if (facet === undefined)
    throw new QueryError("Unknown " + kind + " “" + value + "”. Choose a " + kind + " from the suggestions.");
  if (facet === null)
    throw new QueryError("Ambiguous " + kind + " “" + value + "”. Choose a specific " + kind + " from the suggestions.");
  return facet;
}

/** Swap internal identifiers in a shared query for their readable labels. */
function readableQuery(raw, facets) {
  let result = raw;
  try {
    for (const clause of parseQuery(raw).clauses.reverse()) {
      if (clause.kind !== "facet" || !clause.field) continue;
      const values = facets[clause.field] || [];
      const facet = values.find((candidate) => candidate.value === clause.value);
      if (!facet) continue;
      const alias = cachedIndex(values, clause.field).alias(facet);
      if (alias === undefined || alias === facet.value) continue;
      result =
        result.slice(0, clause.start) +
        (clause.exclude ? "-" : "") +
        clause.field +
        ':"' + alias.replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"' +
        result.slice(clause.end);
    }
  } catch {
    return raw;
  }
  return result;
}

/* ------------------------------------------------------------------ completion */

/** The token under the cursor, which is the only part completion may replace. */
function completionToken(query, cursor) {
  const token = tokenize(query, true).find((entry) => entry.start <= cursor && cursor <= entry.end);
  const start = token?.start ?? cursor;
  const end = token?.end ?? cursor;
  const decoded = tokenize(query.slice(start, cursor), true)[0]?.value || "";
  const excluded = decoded.startsWith("-") ? "-" : "";
  return { token, start, end, excluded, value: excluded ? decoded.slice(1) : decoded };
}

const validToken = (raw, now) => {
  try {
    parseQuery(raw, now);
    return true;
  } catch {
    return false;
  }
};

const facetInsertion = (kind, facet, alias, excluded) =>
  excluded +
  kind +
  ":" +
  (alias === undefined || alias === facet.value
    ? quoteValue(facet.value)
    : '"' + alias.replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"');

/**
 * Suggestions for the token under the cursor: qualifier names, facet values with their counts, and
 * date shortcuts. Articles are never suggested; they belong in the result list.
 */
function complete(query, cursor, facets, now = Date.now(), catalogue = facets) {
  const { token, start, end, excluded, value } = completionToken(query, cursor);
  const finished = token && cursor === token.end && validToken(token.raw, now);

  const field = value.match(/^(source|category|tag|type):(.*)$/i);
  if (field) {
    const kind = field[1].toLowerCase();
    const fragment = normalized(field[2]);
    const identities = cachedIndex(catalogue?.[kind] || [], kind);
    if (finished && identities.lookup(field[2])) return [];
    return cachedIndex(facets?.[kind] || [], kind)
      .entries.filter((entry) => entry.search.includes(fragment))
      .map(({ facet }) => {
        const alias = identities.alias(facet);
        return {
          id: kind + ":" + facet.value,
          label: facet.label,
          detail: alias === undefined ? kind + " · " + facet.value : kind,
          count: facet.count,
          insert: facetInsertion(kind, facet, alias, excluded),
          start,
          end,
        };
      });
  }

  const date = value.match(/^(date|before|after|since|until):(.*)$/i);
  if (date) {
    const kind = date[1].toLowerCase();
    const argument = date[2].toLowerCase();
    if (finished && !argument.endsWith("..")) return [];
    const relative =
      kind === "date"
        ? DATE_SHORTCUTS.filter((value) => value.startsWith(argument)).map((value) => ({
            id: "date:" + value,
            label: value,
            detail: "UTC publication date",
            insert: canonicalQuery(excluded + "date:" + value, now),
            start,
            end,
          }))
        : [];
    const range = argument.lastIndexOf("..");
    const boundary =
      range >= 0 ? argument.slice(0, range + 2) : argument.match(/^(>=|<=|>|<|=)/)?.[0] || "";
    const fragment = argument.slice(boundary.length);
    const days = (facets?.["published-day"] || [])
      .filter((entry) => entry.value.startsWith(fragment))
      .sort((a, b) => b.value.localeCompare(a.value))
      .map((entry) => ({
        id: kind + ":" + boundary + entry.value,
        label: boundary + entry.value,
        detail: "UTC publication date",
        count: entry.count,
        insert: excluded + kind + ":" + boundary + entry.value,
        start,
        end,
      }))
      .filter((entry) => validToken(entry.insert, now));
    return [...relative, ...days];
  }

  const sort = value.match(/^sort:(.*)$/i);
  if (sort)
    return finished || excluded
      ? []
      : ["relevance", "newest", "oldest"]
          .filter((option) => option.startsWith(sort[1].toLowerCase()))
          .map((option) => ({
            id: "sort:" + option,
            label: option,
            detail: "Sort results",
            insert: "sort:" + option,
            start,
            end,
          }));

  if (value.includes(":")) return [];
  return OPERATORS.filter((operator) => operator.startsWith(value.toLowerCase())).map((operator) => ({
    id: operator,
    label: operator,
    detail: "",
    insert: excluded + operator,
    start,
    end,
  }));
}

/** Replace only the token under the cursor, leaving the rest of the query alone. */
function acceptCompletion(query, completion) {
  const tail = query.slice(completion.end);
  const space = completion.insert.endsWith(":") || /^\s/.test(tail) ? "" : " ";
  return {
    query: query.slice(0, completion.start) + completion.insert + space + tail,
    cursor: completion.start + completion.insert.length + space.length,
  };
}

/* ------------------------------------------------------------------ display data */

/**
 * Display metadata travels hex-encoded in a zero-weight Pagefind field, so provider names and
 * JSON keys can never become search terms.
 */
function decodeDisplay(result) {
  try {
    const hex = result.meta?.aggr_display || "";
    if (!/^(?:[0-9a-f]{2})+$/i.test(hex)) return {};
    const bytes = Uint8Array.from(hex.match(/../g) || [], (part) => Number.parseInt(part, 16));
    const value = JSON.parse(new TextDecoder().decode(bytes));
    return value && typeof value === "object" && !Array.isArray(value) ? value : {};
  } catch {
    return {};
  }
}

const displays = new WeakMap();
function displayData(result) {
  let display = displays.get(result);
  if (!display) displays.set(result, (display = decodeDisplay(result)));
  return display;
}

function safeURL(value, base) {
  try {
    const url = new URL(value || "", base);
    return ["http:", "https:"].includes(url.protocol) ? url.href : base;
  } catch {
    return base;
  }
}

/** Only a validated inline PNG may become a placeholder background. */
const placeholderBackground = (value) =>
  value && value.length <= 8192 && /^data:image\/png;base64,[A-Za-z0-9+/]+={0,2}$/.test(value)
    ? 'url("' + value + '")'
    : undefined;

/* ------------------------------------------------------------------ result markup */

const element = (tag, attributes = {}, children = []) => {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) {
    if (value === undefined || value === null || value === false) continue;
    if (name === "text") node.textContent = String(value);
    else node.setAttribute(name, String(value));
  }
  for (const child of children) if (child) node.appendChild(child);
  return node;
};

const field = (children) => element("span", { class: "meta-field" }, children);

/** The same fields, in the same order, as `_metadata.html`. */
function renderMetadata(display, base, original, dates) {
  const fields = [];
  if (display.source_display) {
    const link = element(
      "a",
      {
        class: "domain",
        href: facetPage(base, "source", display.source_slug || ""),
        title: display.source_title || display.source_display,
      },
      [element("span", { class: "source-resolved", text: display.source_display })],
    );
    if (display.is_aggregated) {
      link.append(" ");
      link.appendChild(element("em", { text: "via " + display.feed_display }));
    }
    fields.push(field([link]));
  }
  if (display.category)
    fields.push(
      field([
        element("span", { class: "category" }, [
          element("a", {
            class: "p-category",
            rel: "tag",
            href: facetURL(base, "category", display.category.slug),
            text: "/" + display.category.name,
          }),
        ]),
      ]),
    );
  if (display.date) {
    const updated =
      display.updated &&
      Number.isFinite(Date.parse(display.updated)) &&
      Date.parse(display.updated) !== Date.parse(display.date)
        ? display.updated
        : undefined;
    const tooltip = "Published: " + display.date + (updated ? "\nUpdated: " + updated : "");
    const time = element("time", {
      class: "dt-published",
      datetime: display.date,
      text: dates.text(Date.parse(display.date), dates.format()) || display.date.slice(0, 10),
    });
    fields.push(
      field([
        element(
          "span",
          { class: "published-date", "data-date-tooltip": "", "data-date-updated": updated, title: tooltip },
          [time],
        ),
      ]),
    );
  }
  if (display.consumption) {
    const consumption = display.consumption;
    const stats = element("span", {
      class: "reading-stats",
      "data-consumption": consumption.action,
      "data-duration-seconds": consumption.seconds,
      title: consumption.words
        ? consumption.words + (consumption.words === 1 ? " word" : " words")
        : undefined,
    });
    if (consumption.minutes)
      stats.appendChild(
        element("time", {
          datetime: "PT" + (consumption.seconds || consumption.minutes) + (consumption.seconds ? "S" : "M"),
          text: consumption.minutes + " min " + consumption.action,
        }),
      );
    else stats.textContent = consumption.action === "listen" ? "Listen" : "Watch";
    fields.push(field([stats]));
  }
  fields.push(
    field([
      element("a", {
        class: "u-bookmark-of",
        href: original,
        title: original,
        target: "_blank",
        rel: "external noopener noreferrer via",
        text: "original",
      }),
    ]),
  );
  for (const discussion of display.discussions || [])
    fields.push(
      field([
        element("a", {
          class: "discussion",
          "data-discussion": discussion.name,
          href: safeURL(discussion.url, base),
          title: discussion.url,
          target: "_blank",
          rel: "noopener noreferrer",
          "aria-label":
            discussion.name +
            ", matching discussion found" +
            (discussion.score !== undefined ? ", score " + discussion.score : ""),
          text: discussion.name,
        }),
      ]),
    );
  if (display.points !== undefined)
    fields.push(field([element("span", { text: display.points + " points" })]));
  if (display.comments)
    fields.push(
      field([
        element("a", {
          href: safeURL(display.comments.url, base),
          title: display.comments.url,
          target: "_blank",
          rel: "noopener noreferrer",
          text: display.comments.count !== undefined ? display.comments.count + " comments" : "comments",
        }),
      ]),
    );
  return element("div", { class: "meta" }, fields);
}

/** Pagefind's excerpt comes as HTML; rebuild it as text nodes so nothing can inject markup. */
function renderExcerpt(result) {
  const container = element("div", { class: "search-excerpt" });
  const html = result.excerpt;
  if (!html) {
    container.textContent = displayData(result).excerpt || "";
    return container;
  }
  const parsed = new DOMParser().parseFromString(html, "text/html");
  const walker = parsed.createTreeWalker(parsed.body, NodeFilter.SHOW_TEXT);
  while (walker.nextNode()) {
    const node = walker.currentNode;
    if (node.parentElement?.closest("script,style")) continue;
    const text = node.textContent || "";
    if (node.parentElement?.closest("mark")) container.appendChild(element("mark", { text }));
    else container.appendChild(document.createTextNode(text));
  }
  return container;
}

/** One result row, matching the structure of `_item.html`. */
function renderRow(result, context) {
  const display = displayData(result);
  const page = safeURL(result.url, context.base);
  const original = safeURL(display.original || result.url, context.base);

  const heading = element("div", { class: "row-heading" }, [
    element("span", { class: "rank", "aria-hidden": "true" }),
    element("a", {
      class: "title p-name u-url",
      "data-row-open": "",
      href: page,
      text: result.meta?.title || "Untitled",
    }),
  ]);

  const copy = element("div", { class: "row-copy" }, [
    heading,
    renderExcerpt(result),
    renderMetadata(display, context.base, original, context.dates),
  ]);

  const content = element("div", { class: "row-content" }, [copy]);
  const preview = display.preview;
  if (preview) {
    const media = element("span", {
      class: "preview-media",
      "data-preview-bound": "true",
      "data-thumbhash": preview.placeholder?.hash,
    });
    const background = placeholderBackground(preview.placeholder?.data_url);
    if (background) media.style.setProperty("--image-preview", background);
    if (/^#[0-9a-f]{6}$/i.test(preview.color || "")) media.style.setProperty("--preview-color", preview.color);
    media.appendChild(
      element("img", {
        class: "preview-image",
        src: safeURL(preview.url, context.base),
        width: preview.width,
        height: preview.height,
        alt: preview.alt || "",
        loading: "lazy",
        decoding: "async",
      }),
    );
    content.appendChild(media);
  }

  return element("li", { class: "row h-entry", "data-url": page, "data-link": original }, [
    element("a", { class: "u-uid u-url", href: page, hidden: "", "aria-label": result.meta?.title }),
    element("div", { class: "cell" }, [content]),
  ]);
}

/* ------------------------------------------------------------------ engine */

/** Turn the parsed query's facet and date clauses into Pagefind's filter shape. */
function buildFilters(query, catalogue) {
  const filters = {};
  for (const field of FACET_FIELDS) {
    const clauses = query.clauses.filter((clause) => clause.kind === "facet" && clause.field === field);
    const resolve = (value) => resolveFacet(value, catalogue.facets[field] || [], field).value;
    const any = [...new Set(clauses.filter((clause) => !clause.exclude).map((clause) => resolve(clause.value)))];
    const none = [...new Set(clauses.filter((clause) => clause.exclude).map((clause) => resolve(clause.value)))];
    if (any.length || none.length)
      filters[field] = { ...(any.length ? { any } : {}), ...(none.length ? { none } : {}) };
  }
  const dates = query.clauses.filter((clause) => clause.kind === "date");
  if (dates.length) {
    const days = (catalogue.facets["published-day"] || [])
      .map((facet) => facet.value)
      .filter((value) => dates.every((clause) => dateMatches(clause, value)));
    filters["published-day"] = { any: days.length ? days : ["__no_published_day__"] };
  }
  return filters;
}

function createEngine(base) {
  /** @type {Promise<any> | undefined} */
  let catalogue;
  /** @type {Promise<any> | undefined} */
  let api;
  const searches = new Map();
  const hydrated = new Map();
  const loading = new Map();

  /** The catalogue is fetched fresh so a cached page never points at a retired index. */
  function loadCatalogue() {
    catalogue ??= fetch(new URL("search-catalog.json", base).href, { cache: "no-store" })
      .then((response) => {
        if (!response.ok) throw new Error("search catalogue unavailable");
        return response.json();
      })
      .catch((error) => {
        catalogue = undefined;
        throw error;
      });
    return catalogue;
  }

  async function instance() {
    const manifest = await loadCatalogue();
    api ??= (async () => {
      const bundle = new URL(manifest.base, base);
      if (bundle.origin !== new URL(base).origin) throw new Error("search index is off-origin");
      const module = await import(new URL("pagefind.js", bundle).href);
      const instance = module.createInstance({
        basePath: bundle.pathname,
        baseUrl: new URL(base).pathname,
        excerptLength: 28,
        ranking: {
          termFrequency: 0.65,
          termSimilarity: 1,
          pageLength: 0.35,
          termSaturation: 0.8,
          // Display data carries weight zero so it cannot influence ranking.
          metaWeights: { title: 12, source: 2, date: 0, aggr_display: 0 },
        },
      });
      // An instance cannot answer anything until it has loaded its entry table.
      try {
        await instance.init();
      } catch (error) {
        await instance.destroy?.().catch(() => {});
        throw error;
      }
      return instance;
    })().catch((error) => {
      api = undefined;
      throw error;
    });
    return { manifest, api: await api };
  }

  /** Repeat searches within a session are common; keep a bounded cache of their ID sets. */
  function search(client, term, options) {
    const key = JSON.stringify([term, options]);
    let pending = searches.get(key);
    if (!pending) {
      pending = client.search(term, options).catch((error) => {
        searches.delete(key);
        throw error;
      });
      searches.set(key, pending);
      if (searches.size > 32) searches.delete(searches.keys().next().value);
    }
    return pending;
  }

  /**
   * Pagefind reuses one mutable fragment per document, so copy an immutable snapshot before
   * another query can hydrate the same document. Unrelated documents still load in parallel.
   */
  function hydrate(client, result) {
    let data = hydrated.get(result);
    if (data) return data;
    const previous = loading.get(result.id) ?? Promise.resolve();
    data = previous
      .then(() => result.data())
      .then((value) => ({
        url: value.url,
        meta: { ...value.meta },
        ...(value.excerpt !== undefined ? { excerpt: value.excerpt } : {}),
      }))
      .catch((error) => {
        if (hydrated.get(result) === data) hydrated.delete(result);
        throw error;
      });
    const settled = data.then(
      () => {},
      () => {},
    );
    loading.set(result.id, settled);
    void settled.then(() => {
      if (loading.get(result.id) === settled) loading.delete(result.id);
    });
    hydrated.set(result, data);
    if (hydrated.size > 256) hydrated.delete(hydrated.keys().next().value);
    return data;
  }

  /**
   * Every phrase intersection and exclusion is applied to complete result-ID sets, so counts and
   * paging can never depend on a first-page sample.
   */
  async function matching(client, query, manifest) {
    const terms = query.clauses.filter((clause) => clause.kind === "text" && !clause.exclude);
    const plain = terms.filter((clause) => !clause.quoted).map((clause) => clause.value).join(" ");
    const phrases = terms
      .filter((clause) => clause.quoted)
      .map((clause) => '"' + clause.value.replace(/"/g, "") + '"');
    const negative = query.clauses
      .filter((clause) => clause.kind === "text" && clause.exclude)
      .map((clause) => (clause.quoted ? '"' + clause.value.replace(/"/g, "") + '"' : clause.value));

    // A filter-only query has no relevance to sort by, so it falls back to newest.
    const sort = query.sort === "relevance" && !terms.length ? "newest" : query.sort;
    const options = { filters: buildFilters(query, manifest) };
    if (sort !== "relevance") options.sort = { date: sort === "oldest" ? "asc" : "desc" };

    const positive = [...(plain ? [plain] : []), ...phrases];
    if (!positive.length) positive.push(null);
    const matches = await Promise.all(
      [...positive, ...negative].map((term) => search(client, term, options)),
    );
    const required = matches
      .slice(1, positive.length)
      .map((match) => new Set(match.results.map((result) => result.id)));
    const excluded = new Set(
      matches.slice(positive.length).flatMap((match) => match.results.map((result) => result.id)),
    );
    const results = matches[0].results.filter(
      (result) => required.every((ids) => ids.has(result.id)) && !excluded.has(result.id),
    );
    return { results, filters: matches.length === 1 ? matches[0].filters : undefined };
  }

  return {
    catalogue: loadCatalogue,
    /** Load the catalogue, the engine and its entry table now; a query should only have to ask. */
    warm() {
      void instance().catch(() => {});
    },
    async run(query, requestedPage, size) {
      const { manifest, api: client } = await instance();
      const { results } = await matching(client, query, manifest);
      const pages = Math.max(1, Math.ceil(results.length / size));
      const page = Math.max(1, Math.min(pages, requestedPage));
      return {
        results: await Promise.all(
          results.slice((page - 1) * size, page * size).map((result) => hydrate(client, result)),
        ),
        total: results.length,
        page,
        pages,
        size,
      };
    },
    /** Counts for one facet, scoped to every other clause still in the query. */
    async counts(query, field) {
      const { manifest, api: client } = await instance();
      const matches = await matching(client, query, manifest);
      const facets = manifest.facets[field] || [];
      if (matches.filters?.[field])
        return facets
          .map((facet) => ({ ...facet, count: matches.filters[field][facet.value] || 0 }))
          .filter((facet) => facet.count > 0);
      const ids = new Set(matches.results.map((result) => result.id));
      if (!ids.size) return [];
      // A compound text query has no single count map; intersect filter memberships instead of
      // hydrating every match merely to count.
      const counted = await Promise.all(
        facets.map(async (facet) => {
          const members = await search(client, null, { filters: { [field]: facet.value } });
          return {
            ...facet,
            count: members.results.reduce((total, result) => total + Number(ids.has(result.id)), 0),
          };
        }),
      );
      return counted.filter((facet) => facet.count > 0);
    },
  };
}

/* ------------------------------------------------------------------ controller */

export function mount(options) {
  const base = options.base;
  const dates = options.dates;
  const preferences = options.preferences;
  const root = $("[data-search-root]");
  const input = /** @type {HTMLInputElement | null} */ ($("#q"));
  const form = $("#search-form");
  const results = $("[data-search-results]");
  if (!root || !input || !form || !results) return;

  const staticFeed = $("[data-static-feed]");
  const listbox = $("#search-completions");
  const clear = $(".search-clear");
  const status = $("#search-status");
  const list = $("#list");
  const empty = $("#empty");
  const pager = $(".search-pager");
  const error = $(".search-error", root);

  const engine = createEngine(base);
  // The index is what a search waits for, so start it with the page rather than with the query.
  engine.warm();
  const scope = root.dataset.scopeKind
    ? root.dataset.scopeKind + ":" + root.dataset.scopeValue
    : "";

  let generation = 0;
  let suggestions = [];
  let highlighted = 0;
  let menuOpen = false;
  let composing = false;
  let debounce;
  let page = 1;

  const pageSize = () => Number(preferences?.values["feed-page-size"]) || 50;
  const active = () => input.value.trim() !== "" && input.value.trim() !== scope;

  function showError(message) {
    if (!error) return;
    error.textContent = message || "";
    error.hidden = !message;
  }

  function closeMenu() {
    menuOpen = false;
    if (listbox) {
      listbox.hidden = true;
      // Drop the options too: a hidden list still answers queries and reads to assistive
      // technology, so a stale suggestion would outlive the keystroke that produced it.
      listbox.replaceChildren();
    }
    input.setAttribute("aria-expanded", "false");
    input.removeAttribute("aria-activedescendant");
  }

  function renderMenu() {
    if (!listbox) return;
    if (!menuOpen || !suggestions.length) return closeMenu();
    listbox.replaceChildren(
      ...suggestions.map((suggestion, index) => {
        const detail =
          suggestion.detail + (suggestion.count !== undefined ? " · " + suggestion.count : "");
        const item = element(
          "li",
          {
            class: "search-completion",
            role: "option",
            id: "search-completion-" + index,
            "data-completion-id": suggestion.id,
            "aria-selected": String(index === highlighted),
            ...(index === highlighted ? { "data-selected": "" } : {}),
          },
          [
            element("span", { class: "completion-label", text: suggestion.label }),
            detail.trim() ? element("small", { text: detail.trim() }) : null,
          ],
        );
        item.addEventListener("pointerdown", (event) => {
          event.preventDefault();
          choose(suggestion);
        });
        item.addEventListener("pointermove", () => {
          if (highlighted === index) return;
          highlighted = index;
          renderMenu();
        });
        return item;
      }),
    );
    listbox.hidden = false;
    input.setAttribute("aria-expanded", "true");
    input.setAttribute("aria-activedescendant", "search-completion-" + highlighted);
  }

  /** Counts for the facet being completed, scoped to the query's other clauses. */
  let context = null;
  let contextGeneration = 0;

  /** The facet qualifier under the cursor, and the query with that token taken out. */
  function editedFacet() {
    const cursor = input.selectionStart ?? input.value.length;
    const { start, end, value } = completionToken(input.value, cursor);
    const field = value.match(/^(source|category|tag|type):/i)?.[1]?.toLowerCase();
    if (!field) return null;
    return { field, rest: (input.value.slice(0, start) + input.value.slice(end)).trim() };
  }

  /**
   * Offer only the values that would actually narrow the current search, with the number of
   * articles each would leave. An older context must never replace a newer one.
   */
  async function refineContext() {
    const edited = editedFacet();
    // With nothing else in the query the catalogue's own counts are already the contextual ones.
    if (!edited || !edited.rest) {
      contextGeneration++;
      if (context) {
        context = null;
        renderSuggestions();
      }
      return;
    }
    if (context && context.field === edited.field && context.rest === edited.rest) return;
    const generation = ++contextGeneration;
    try {
      const values = await engine.counts(parseQuery(edited.rest || ""), edited.field);
      if (generation !== contextGeneration) return;
      context = { ...edited, values };
      renderSuggestions();
    } catch {
      /* the whole-archive vocabulary is still a useful answer */
    }
  }

  function renderSuggestions() {
    if (!listbox) return;
    const facets =
      context && catalogueFacets
        ? { ...catalogueFacets, [context.field]: context.values }
        : catalogueFacets;
    try {
      suggestions = complete(
        input.value,
        input.selectionStart ?? input.value.length,
        facets,
        Date.now(),
        catalogueFacets,
      );
    } catch {
      suggestions = [];
    }
    highlighted = 0;
    renderMenu();
  }

  function suggest() {
    if (!listbox) return;
    // A facet cannot be completed without the vocabulary. Fetch it once and come back rather
    // than leaving the reader with an empty menu for a qualifier that does have values.
    if (!catalogueFacets) void loadFacets().then((manifest) => manifest && suggest());
    renderSuggestions();
    void refineContext();
  }

  function choose(suggestion) {
    const accepted = acceptCompletion(input.value, suggestion);
    input.value = accepted.query;
    input.setSelectionRange(accepted.cursor, accepted.cursor);
    closeMenu();
    schedule(0);
    suggest();
  }

  /** Keep `?q=` and `?search-page=` shareable without adding a history entry per keystroke. */
  function syncLocation() {
    try {
      const url = active() ? queryURL(location.href, input.value, page) : stripSearch();
      history.replaceState(history.state, "", url);
    } catch {
      /* an unparseable query is not worth an address-bar update */
    }
  }

  function stripSearch() {
    const url = new URL(location.href);
    url.searchParams.delete("q");
    url.searchParams.delete("search-page");
    return url.href;
  }

  function reset() {
    generation++;
    page = 1;
    document.body.removeAttribute("data-searching");
    if (staticFeed) staticFeed.hidden = false;
    if (list) {
      list.hidden = true;
      list.replaceChildren();
    }
    if (status) status.hidden = true;
    if (empty) empty.hidden = true;
    if (pager) pager.hidden = true;
    showError("");
    syncLocation();
  }

  let catalogueFacets;
  function loadFacets() {
    return engine
      .catalogue()
      .then((manifest) => {
        catalogueFacets = manifest.facets;
        // A query shared with raw identifiers reads better once the vocabulary is known, but
        // never overwrite something the reader has since typed.
        if (!edited && active()) {
          const readable = readableQuery(input.value, catalogueFacets);
          if (readable !== input.value) input.value = readable;
        }
        return manifest;
      })
      .catch(() => undefined);
  }

  async function run() {
    const token = ++generation;
    if (!active()) return reset();

    document.body.setAttribute("data-searching", "");
    if (staticFeed) staticFeed.hidden = true;
    if (status) {
      status.hidden = false;
      status.textContent = "Searching…";
    }

    /** Keep an explanation on screen without leaving results from an earlier query below it. */
    const explain = (message) => {
      showError(message);
      if (list) {
        list.hidden = true;
        list.replaceChildren();
      }
      if (status) status.hidden = true;
      if (empty) empty.hidden = true;
      if (pager) pager.hidden = true;
    };

    let query;
    try {
      query = parseQuery(input.value);
    } catch (failure) {
      explain(failure instanceof Error ? failure.message : "This search could not be read.");
      return;
    }
    showError("");

    try {
      const outcome = await engine.run(query, page, pageSize());
      if (token !== generation) return;
      page = outcome.page;
      renderResults(outcome);
      syncLocation();
    } catch (failure) {
      if (token !== generation) return;
      // A query the reader can fix reads as advice; anything else is a fault worth reporting.
      if (!(failure instanceof QueryError)) console.error("aggr: search", failure);
      explain(
        failure instanceof QueryError
          ? failure.message
          : "Search is unavailable right now. Try again in a moment.",
      );
    }
  }

  function renderResults(outcome) {
    if (!list) return;
    const context = { base, dates };
    list.replaceChildren(...outcome.results.map((result) => renderRow(result, context)));
    list.style.setProperty("--rank-indent", Math.max(2, String(outcome.total).length) + "ch");
    // Rank numbers continue across pages.
    list.style.setProperty("counter-reset", "rank " + (outcome.page - 1) * outcome.size);
    list.hidden = false;
    if (status) {
      status.hidden = false;
      status.textContent = outcome.total + (outcome.total === 1 ? " article" : " articles");
    }
    if (empty) empty.hidden = outcome.total !== 0;
    if (pager) {
      pager.hidden = outcome.pages <= 1;
      const previous = /** @type {HTMLAnchorElement | null} */ ($("[data-search-page-previous]", pager));
      const next = /** @type {HTMLAnchorElement | null} */ ($("[data-search-page-next]", pager));
      const label = $("[data-search-page-status]", pager);
      if (previous) {
        previous.hidden = outcome.page <= 1;
        previous.href = queryURL(location.href, input.value, outcome.page - 1);
      }
      if (next) {
        next.hidden = outcome.page >= outcome.pages;
        next.href = queryURL(location.href, input.value, outcome.page + 1);
      }
      if (label) label.textContent = "page " + outcome.page + " / " + outcome.pages;
    }
    options.selection?.restore();
    dates.render(list);
  }

  function schedule(delay = DEBOUNCE) {
    clearTimeout(debounce);
    debounce = setTimeout(() => void run(), delay);
  }

  let edited = false;

  input.addEventListener("input", () => {
    if (composing) return;
    edited = true;
    page = 1;
    if (clear) clear.hidden = !input.value;
    menuOpen = true;
    suggest();
    schedule();
  });
  input.addEventListener("compositionstart", () => (composing = true));
  input.addEventListener("compositionend", () => {
    composing = false;
    input.dispatchEvent(new Event("input"));
  });
  input.addEventListener("focus", () => {
    if (clear) clear.hidden = !input.value;
    menuOpen = true;
    suggest();
  });
  input.addEventListener("blur", () => setTimeout(closeMenu, 120));
  input.addEventListener("keyup", (event) => {
    if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) suggest();
  });
  input.addEventListener("click", () => suggest());

  input.addEventListener("keydown", (event) => {
    if (composing || event.isComposing) {
      event.stopPropagation();
      return;
    }
    const suggesting = menuOpen && suggestions.length > 0;
    if (event.key === "Escape") {
      // The first Escape closes the suggestions; the next leaves the field. Neither clears it.
      event.preventDefault();
      event.stopPropagation();
      if (suggesting) closeMenu();
      else input.blur();
      return;
    }
    if ((event.key === "Enter" || (event.key === "Tab" && !event.shiftKey)) && suggesting) {
      event.preventDefault();
      event.stopPropagation();
      choose(suggestions[highlighted] || suggestions[0]);
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      if (suggesting) {
        event.preventDefault();
        event.stopPropagation();
        highlighted =
          (highlighted + (event.key === "ArrowDown" ? 1 : -1) + suggestions.length) % suggestions.length;
        renderMenu();
        return;
      }
      // Without suggestions the arrows walk the results, and the keyboard stays in the field.
      if (options.selection?.move(event.key === "ArrowDown" ? 1 : -1, false)) {
        event.preventDefault();
        event.stopPropagation();
      }
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      event.stopPropagation();
      closeMenu();
      // With no suggestion to accept, Enter opens the result the cursor is on.
      const link = options.selection?.link(options.selection.selected());
      if (link) link.click();
      else schedule(0);
    }
  });

  clear?.addEventListener("click", () => {
    input.value = "";
    edited = true;
    clear.hidden = true;
    closeMenu();
    reset();
    input.focus({ preventScroll: true });
  });

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    closeMenu();
    schedule(0);
  });

  pager?.addEventListener("click", (event) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const link = target.closest("[data-search-page-previous], [data-search-page-next]");
    if (!link) return;
    event.preventDefault();
    page += link.hasAttribute("data-search-page-next") ? 1 : -1;
    void run();
    results.scrollIntoView({ block: "start", behavior: "instant" });
  });

  // A shared URL or a Back navigation must restore the same results.
  window.addEventListener("popstate", () => {
    const url = new URL(location.href);
    input.value = url.searchParams.get("q") || input.value;
    page = Number(url.searchParams.get("search-page")) || 1;
    void run();
  });

  if (clear) clear.hidden = !input.value;
  void loadFacets().then(() => suggest());
  const url = new URL(location.href);
  if (url.searchParams.has("q")) {
    input.value = url.searchParams.get("q") || "";
    page = Number(url.searchParams.get("search-page")) || 1;
  }
  // This module is fetched on the first sign of interest, so the reader may already have typed
  // by the time it arrives. A shared `?q=` link lands here too.
  if (active()) void run();
  // If they are still in the field, show them the suggestions for what they have typed rather
  // than waiting for another keystroke.
  if (document.activeElement === input) {
    menuOpen = true;
    suggest();
  }
}
