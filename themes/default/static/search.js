// @ts-check
/**
 * Full-text search with a compact catalogue and a lazily initialized index.
 *
 * Pagefind builds and serves the index; everything here is the query language, the completion
 * menu and the result list on top of it. The chrome is already in the HTML: this file fills it.
 */

/** @typedef {import('../../../types/search').Query} Query */
/** @typedef {import('../../../types/search').Clause} Clause */
/** @typedef {import('../../../types/search').Facet} Facet */
/** @typedef {import('../../../types/search').Facets} Facets */
/** @typedef {import('../../../types/search').Catalog} Catalog */
/** @typedef {import('../../../types/search').Completion} Completion */
/** @typedef {import('../../../types/search').Result} Result */
/** @typedef {import('../../../types/search').ResultRef} ResultRef */
/** @typedef {import('../../../types/search').Pagefind} Pagefind */
/** @typedef {import('../../../types/search').Outcome} Outcome */
/** @typedef {import('../../../types/search').Display} Display */
/** @param {string} selector @param {ParentNode} [root] */
const $ = (selector, root = document) => root.querySelector(selector);

const FACET_FIELDS = ["source", "category", "tag", "type"];
const DATE_FIELDS = ["date", "before", "after", "since", "until"];
const OPERATORS = [
  "source:", "category:", "tag:", "type:", "date:", "after:", "before:", "since:", "until:", "sort:",
];
const DATE_SHORTCUTS = ["today", "yesterday", "last7d", "last30d", "year"];
const MAX_CHARS = 4096;
const MAX_CLAUSES = 16;
/** Typing pause before a query runs. Long enough to coalesce a burst, short enough to feel live:
    the index is already warm, stale runs are discarded by generation, and repeats are cached. */
const DEBOUNCE = 60;

class QueryError extends Error {
  /** @param {string} message */
  constructor(message, start = 0, end = start) {
    super(message);
    this.start = start;
    this.end = end;
  }
}

/* ------------------------------------------------------------------ query language */

/** Split a query into clauses, honouring quoted phrases and backslash escapes. */
/** @param {string} raw */
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

/** @param {string} value */
const quoteValue = (value) =>
  /[\s"\\]/u.test(value) ? '"' + value.replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"' : value;

const DAY = 86400000;
/** @param {number} timestamp */
const day = (timestamp) => new Date(timestamp).toISOString().slice(0, 10);

/** @param {string} value */
function checkedDay(value) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value) || !Number.isFinite(Date.parse(value)) || day(Date.parse(value)) !== value)
    throw new QueryError("Use a valid UTC date, such as 2026-09-08.");
  return value;
}
/** @param {string} value @param {number} days */
const shifted = (value, days) => day(Date.parse(value) + days * DAY);

/** Resolve a date qualifier to an inclusive range of UTC publication days. */
/** @param {string} value @param {string} operator @param {number} now @returns {{from?: string, through?: string}} */
function dateRange(value, operator, now) {
  const today = day(now);
  if (operator === "date") {
    if (value === "today") return { from: today, through: today };
    if (value === "yesterday") return { from: shifted(today, -1), through: shifted(today, -1) };
    const shortcut = /** @type {Record<string, number>} */ ({ week: 7, last7d: 7, month: 30, last30d: 30, year: 365 })[value];
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
        /** @type {Record<string, string>} */ ({ ">=": "since", "<=": "until", ">": "after", "<": "before", "=": "date" })[comparison[1]],
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

/** @param {string} raw @returns {Query} */
function parseQuery(raw, now = Date.now()) {
  if (raw.length > MAX_CHARS)
    throw new QueryError("Search is limited to 4,096 characters.", MAX_CHARS, raw.length);
  const tokens = tokenize(raw);
  if (tokens.length > MAX_CLAUSES)
    throw new QueryError("Use at most 16 search clauses.", tokens[MAX_CLAUSES].start, raw.length);
  let sort = "relevance";
  const clauses = tokens.map(/** @returns {Clause} */ (token) => {
    const exclude = token.value.startsWith("-") && !token.raw.startsWith('"');
    const value = exclude ? token.value.slice(1) : token.value;
    if (!value.trim())
      throw new QueryError("Add a word or phrase to search for.", token.start, token.end);
    /** @type {Clause} */
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

/** @param {{from?: string, through?: string, exclude?: boolean}} clause @param {string} date */
function dateMatches(clause, date) {
  const matches = (!clause.from || date >= clause.from) && (!clause.through || date <= clause.through);
  return clause.exclude ? !matches : matches;
}

/** Replace relative date shortcuts with absolute dates, so a shared link stays stable. */
/** @param {string} query */
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

/** @param {string} base @param {string} query */
function queryURL(base, query, page = 1, now = Date.now()) {
  const url = new URL(base);
  if (query.trim()) url.searchParams.set("q", canonicalQuery(query, now));
  else url.searchParams.delete("q");
  if (page > 1) url.searchParams.set("search-page", String(page));
  else url.searchParams.delete("search-page");
  return url.href;
}

/** @param {string} base @param {string} field @param {string} value */
const facetURL = (base, field, value) =>
  queryURL(base, field + ':"' + value.replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"');

/** The collection page a facet already has, matching `facet_page` in the templates. */
/** @param {string} base @param {string} field @param {string} value */
const facetPage = (base, field, value) =>
  new URL((field === "source" ? "sources/" : field === "category" ? "categories/" : "tags/") + value + "/", base)
    .href;

/* ------------------------------------------------------------------ facets */

/** @param {string} value */
const normalized = (value) => value.normalize("NFKC").toLocaleLowerCase();

/**
 * Facet values by identifier and by readable label. A label shared by two values resolves to
 * nothing rather than silently picking one.
 */
/** @param {Facet[]} values @param {string} kind */
function facetIndex(values, kind) {
  /** @type {Map<string, Facet>} */
  const identifiers = new Map();
  /** @type {Map<string, Facet | null>} */
  const labels = new Map();
  for (const facet of values) {
    if (!identifiers.has(facet.value)) identifiers.set(facet.value, facet);
    const label = normalized(facet.label);
    labels.set(label, labels.has(label) ? null : facet);
  }
  /** @param {Facet} facet */
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
    lookup: (/** @type {string} */ value) => identifiers.get(value) ?? labels.get(normalized(value)),
  };
}

/** @type {WeakMap<Facet[], ReturnType<typeof facetIndex>>} */
const indexCache = new WeakMap();
/** @param {Facet[]} values @param {string} kind */
function cachedIndex(values, kind) {
  let index = indexCache.get(values);
  if (!index) indexCache.set(values, (index = facetIndex(values, kind)));
  return index;
}

/** @param {string} value @param {Facet[]} values @param {string} kind */
function resolveFacet(value, values, kind) {
  const facet = cachedIndex(values, kind).lookup(value);
  if (facet === undefined)
    throw new QueryError("Unknown " + kind + " “" + value + "”. Choose a " + kind + " from the suggestions.");
  if (facet === null)
    throw new QueryError("Ambiguous " + kind + " “" + value + "”. Choose a specific " + kind + " from the suggestions.");
  return facet;
}

/** Swap internal identifiers in a shared query for their readable labels. */
/** @param {string} raw @param {Facets} facets */
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
/** @param {string} query @param {number} cursor */
function completionToken(query, cursor) {
  const token = tokenize(query, true).find((entry) => entry.start <= cursor && cursor <= entry.end);
  const start = token?.start ?? cursor;
  const end = token?.end ?? cursor;
  const decoded = tokenize(query.slice(start, cursor), true)[0]?.value || "";
  const excluded = decoded.startsWith("-") ? "-" : "";
  return { token, start, end, excluded, value: excluded ? decoded.slice(1) : decoded };
}

/** @param {string} raw @param {number} now */
const validToken = (raw, now) => {
  try {
    parseQuery(raw, now);
    return true;
  } catch {
    return false;
  }
};

/** @param {string} kind @param {Facet} facet @param {string | undefined} alias @param {string} excluded */
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
/** @param {string} query @param {number} cursor @param {Facets | undefined} facets @param {number} [now] @param {Facets | undefined} [catalogue] @param {boolean} [scoped] @returns {Completion[]} */
function complete(query, cursor, facets, now = Date.now(), catalogue = facets, scoped = false) {
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
          // Name what the clause will actually say, unless the label already says it.
          detail:
            alias === undefined && facet.label !== facet.value
              ? kind + " · " + facet.value
              : kind,
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
        ? DATE_SHORTCUTS.filter(value => value.startsWith(argument)).map(value => {
            const range = { ...dateRange(value, "date", now), exclude: Boolean(excluded) };
            const count = (facets?.["published-day"] || []).filter(day => dateMatches(range, day.value)).reduce((total, day) => total + day.count, 0);
            return { id: "date:" + value, label: value, detail: "UTC publication date",
              insert: canonicalQuery(excluded + "date:" + value, now), start, end,
              ...(scoped ? { count } : {}) };
          }).filter(option => !scoped || option.count)
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
        count: scoped
          ? (facets?.["published-day"] || []).filter(day => {
              try { return dateMatches({ ...dateRange(boundary + entry.value, kind, now), exclude: Boolean(excluded) }, day.value); }
              catch { return false; }
            }).reduce((total, day) => total + day.count, 0)
          : entry.count,
        insert: excluded + kind + ":" + boundary + entry.value,
        start,
        end,
      }))
      .filter((entry) => validToken(entry.insert, now) && (!scoped || entry.count > 0));
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
/** @param {string} query @param {Completion} completion */
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
/** @param {Result} result @returns {Display} */
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

/** @type {WeakMap<Result, Display>} */
const displays = new WeakMap();
/** @param {Result} result @returns {Display} */
function displayData(result) {
  let display = displays.get(result);
  if (!display) displays.set(result, (display = decodeDisplay(result)));
  return display;
}

/** @param {string | undefined} value @param {string} base */
function safeURL(value, base) {
  try {
    const url = new URL(value || "", base);
    return ["http:", "https:"].includes(url.protocol) ? url.href : base;
  } catch {
    return base;
  }
}

/** Only a validated inline PNG may become a placeholder background. */
/** @param {string | undefined} value */
const placeholderBackground = (value) =>
  value && value.length <= 8192 && /^data:image\/png;base64,[A-Za-z0-9+/]+={0,2}$/.test(value)
    ? 'url("' + value + '")'
    : undefined;

/* ------------------------------------------------------------------ result markup */

/** @template {keyof HTMLElementTagNameMap} T @param {T} tag @param {Record<string, unknown>} [attributes] @param {(Node | null | undefined)[]} [children] @returns {HTMLElementTagNameMap[T]} */
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

/** @param {Node[]} children */
const field = (children) => element("span", { class: "meta-field" }, children);

/** The same fields, in the same order, as `_metadata.html`. */
/** @param {Display} display @param {string} base @param {string} original @param {import("../../../types/search").Dates} dates */
function renderMetadata(display, base, original, dates) {
  const fields = [];
  if (display.source_display) {
    const link = element(
      "a",
      {
        href: facetPage(base, "source", display.source_query || display.source_slug || ""),
        title: display.source_title || display.source_display,
      },
      [element("span", { class: "source-resolved", text: display.source_display })],
    );
    const domain = element("span", { class: "domain" }, [link]);
    // The feeds that carried this article are links of their own, exactly as `_metadata.html`
    // renders them: whoever published it and whoever passed it on are both worth following.
    const feeds = display.feed_sources || [];
    if (feeds.length) {
      const via = element("em", {}, [document.createTextNode("via ")]);
      feeds.forEach((feed, index) => {
        if (index) via.append(", ");
        via.appendChild(
          element("a", {
            class: "source-feed",
            href: facetPage(base, "source", feed.query_value || feed.slug),
            title: feed.name,
            text: feed.display,
          }),
        );
      });
      domain.append(" ");
      domain.appendChild(via);
    }
    fields.push(field([domain]));
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
    fields.push(field([element("a", {
      href: safeURL(display.comments.url, base), title: display.comments.url,
      target: "_blank", rel: "noopener noreferrer",
      text: (display.comments.count !== undefined ? display.comments.count + " " : "") + "comments",
    })]));
  return element("div", { class: "meta" }, fields);
}

/**
 * Pagefind's excerpt comes as HTML; rebuild it as text nodes so nothing can inject markup. An
 * excerpt that highlights nothing (a query of filters alone, or one that matched the metadata)
 * says less than the article's own summary, which the feed row shows too.
 */
/** @param {Result} result */
function renderExcerpt(result) {
  const container = element("div", { class: "search-excerpt" });
  const html = result.excerpt;
  if (!html || (!html.includes("<mark") && displayData(result).excerpt)) {
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
/** @param {Result} result @param {{base: string, dates: import("../../../types/search").Dates, cachedPreviews?: Set<string>}} context */
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
  if (window.AGGROffline?.saved.some(saved => safeURL(saved.url, context.base) === page))
    copy.appendChild(element("div", { class: "search-saved-status", "data-saved-offline": "true", text: "Saved offline" }));
  if (preview && (navigator.onLine || context.cachedPreviews?.has(safeURL(preview.url, context.base)))) {
    const media = element("span", {
      class: "preview-media",
      "data-preview-bound": "true",
    });
    const background = placeholderBackground(preview.placeholder?.data_url);
    if (background) media.style.setProperty("--image-preview", background);
    if (preview.color && /^#[0-9a-f]{6}$/i.test(preview.color)) media.style.setProperty("--preview-color", preview.color);
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
/** @param {Query} query @param {Catalog} catalogue @returns {Record<string, {any?: string[], none?: string[]}>} */
function buildFilters(query, catalogue) {
  /** @type {Record<string, {any?: string[], none?: string[]}>} */
  const filters = {};
  for (const field of FACET_FIELDS) {
    const clauses = query.clauses.filter((clause) => clause.kind === "facet" && clause.field === field);
    const resolve = (/** @type {string} */ value) => resolveFacet(value, catalogue.facets[field] || [], field).value;
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

/**
 * The most index work search keeps in flight at once, whoever asks for it. Browsers already queue
 * past six connections to one origin over HTTP/1.1, so more would only move the queue out of reach,
 * and six keep small fragments moving over HTTP/2 as well.
 */
const INDEX_REQUESTS = 6;

/** Run tasks with at most `size` in flight, in the order they were asked for. */
/** @param {number} size */
function limiter(size) {
  let active = 0;
  /** @type {(() => void)[]} */
  const waiting = [];
  const next = () => {
    while (active < size && waiting.length) {
      const task = waiting.shift();
      if (task) { active++; task(); }
    }
  };
  /** @template T @param {() => Promise<T>} task @returns {Promise<T>} */
  return task => new Promise((resolve, reject) => {
    waiting.push(() => {
      Promise.resolve().then(task).then(resolve, reject).finally(() => { active--; next(); });
    });
    next();
  });
}

/** One immutable catalogue and its lazily initialized runtime. Retired runtimes finish their leases. */
/** @param {string} base @param {Catalog} manifest @param {(url: string) => Promise<import("../../../types/search").PagefindModule>} importAPI @param {AbortSignal} lifetime */
function createIndex(base, manifest, importAPI, lifetime) {
  /** @type {Promise<Pagefind> | undefined} */
  let api;
  /** @type {Map<string, Promise<import('../../../types/search').Matches>>} */
  const searches = new Map();
  /** @type {Map<ResultRef, Promise<Result>>} */
  const hydrated = new Map();
  /** @type {Map<string, Promise<void>>} */
  const loading = new Map();
  /** @type {Map<string, Promise<Set<string>>>} */
  const memberships = new Map();
  const limit = limiter(INDEX_REQUESTS);
  let leases = 0;
  let retired = false;
  let destroyed = false;

  function release() {
    if (!retired || leases || destroyed) return;
    destroyed = true;
    void api?.then(client => client.destroy()).catch(() => {});
    searches.clear(); hydrated.clear(); memberships.clear();
  }

  async function instance() {
    api ??= (async () => {
      const bundle = new URL(manifest.base, base);
      const module = await importAPI(new URL("pagefind.js", bundle).href);
      lifetime.throwIfAborted();
      const client = module.createInstance({
        basePath: bundle.pathname, baseUrl: new URL(base).pathname, excerptLength: 28,
        ranking: { termFrequency: 0.65, termSimilarity: 1, pageLength: 0.35,
          termSaturation: 0.8, metaWeights: { title: 12, source: 2, date: 0, aggr_display: 0 } },
      });
      try { await client.init(); }
      catch (error) { await client.destroy().catch(() => {}); throw error; }
      return client;
    })().catch(error => { api = undefined; throw error; });
    return api;
  }

  /** @template T @param {(client: Pagefind) => Promise<T>} task @param {AbortSignal} [signal] */
  async function use(task, signal) {
    signal?.throwIfAborted();
    if (retired) throw new Error("Search index is retired.");
    leases++;
    try { const client = await instance(); signal?.throwIfAborted(); lifetime.throwIfAborted(); return await task(client); }
    finally { leases--; release(); }
  }

  /** Repeat searches within a session are common; keep a bounded cache of their ID sets. */
  /** @param {Pagefind} client @param {string | null} term @param {Record<string, unknown>} options */
  function search(client, term, options) {
    const key = JSON.stringify([term, options]);
    let pending = searches.get(key);
    if (!pending) {
      pending = limit(() => client.search(term, options)).catch((error) => {
        searches.delete(key);
        throw error;
      });
      searches.set(key, pending);
      if (searches.size > 32) searches.delete(searches.keys().next().value || "");
    }
    return pending;
  }

  /**
   * Pagefind reuses one mutable fragment per document, so copy an immutable snapshot before
   * another query can hydrate the same document. Unrelated documents still load in parallel.
   */
  /** @param {Pagefind} client @param {ResultRef} result */
  function hydrate(client, result) {
    let data = hydrated.get(result);
    if (data) return data;
    const previous = loading.get(result.id) ?? Promise.resolve();
    data = previous
      .then(() => limit(() => result.data()))
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
    if (hydrated.size > 256) hydrated.delete(/** @type {ResultRef} */ (hydrated.keys().next().value));
    return data;
  }

  /**
   * Every phrase intersection and exclusion is applied to complete result-ID sets, so counts and
   * paging can never depend on a first-page sample.
   */
  /** @param {Pagefind} client @param {Query} query @param {Catalog} manifest */
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
    /** @type {Record<string, unknown>} */
    const options = { filters: buildFilters(query, manifest) };
    if (sort !== "relevance") options.sort = { date: sort === "oldest" ? "asc" : "desc" };

    /** @type {(string | null)[]} */
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
    return {
      results,
      filters: matches.length === 1 ? matches[0].filters : undefined,
      // Each positive term's own counts. The results are a subset of every one of those searches,
      // so a value any of them never saw cannot occur among the results.
      bounds: matches.slice(0, positive.length).map((match) => match.filters),
    };
  }

  /** The IDs of every document carrying one facet value, fetched once per page. */
  /** @param {Pagefind} client @param {string} field @param {string} value */
  function membership(client, field, value) {
    const key = field + "\u0000" + value;
    let members = memberships.get(key);
    if (!members) {
      // Straight to the index rather than through `search`: kept as bare IDs here, a membership
      // would only evict the query results that cache exists for.
      members = limit(() => client.search(null, { filters: { [field]: value } }))
        .then((found) => new Set(found.results.map((result) => result.id)))
        .catch((error) => {
          memberships.delete(key);
          throw error;
        });
      memberships.set(key, members);
      // Bounded like the other caches. Evicting one costs a single search the next time it is
      // needed, and a count already under way holds its own copy.
      if (memberships.size > 256) memberships.delete(memberships.keys().next().value || "");
    }
    return members;
  }

  return {
    manifest,
    retire() { retired = true; release(); },
    /** @param {Query} query @param {AbortSignal} [signal] */
    prepare(query, signal) {
      buildFilters(query, manifest);
      return use(async client => {
        if (!query.clauses.some(clause => clause.kind === "text"))
          await client.preload(null, { filters: buildFilters(query, manifest) });
      }, signal);
    },
    /** @param {Query} query @param {number} requestedPage @param {number} size @param {AbortSignal} [signal] */
    run(query, requestedPage, size, signal) {
      return use(async client => {
        const { results } = await matching(client, query, manifest);
        signal?.throwIfAborted();
        const pages = Math.max(1, Math.ceil(results.length / size));
        const page = Math.max(1, Math.min(pages, requestedPage));
        return {
          results: await Promise.all(results.slice((page - 1) * size, page * size).map(result => hydrate(client, result))),
          total: results.length, page, pages, size,
        };
      }, signal);
    },
    /** Counts for one facet after every other clause, without loading article bodies.
     * @param {Query} query @param {string} field @param {AbortSignal} [signal] */
    counts(query, field, signal) {
      return use(async client => {
        const matches = await matching(client, query, manifest);
        signal?.throwIfAborted();
        const facets = manifest.facets[field] || [];
        const counts = matches.filters?.[field];
        if (counts) return facets.map(facet => ({ ...facet, count: counts[facet.value] || 0 })).filter(facet => facet.count > 0);
        const ids = new Set(matches.results.map(result => result.id));
        if (!ids.size) return [];
        const bounds = matches.bounds.flatMap(filters => filters?.[field] ? [filters[field]] : []);
        const candidates = facets.filter(facet => bounds.every(counts => (counts[facet.value] || 0) > 0));
        const counted = await Promise.all(candidates.map(async facet => {
          signal?.throwIfAborted();
          const members = await membership(client, field, facet.value);
          const [small, large] = members.size < ids.size ? [members, ids] : [ids, members];
          let count = 0;
          for (const id of small) if (large.has(id)) count++;
          return { ...facet, count };
        }));
        return counted.filter(facet => facet.count > 0);
      }, signal);
    },
  };
}

/** A shared session keeps catalogue and runtime versions coherent across page navigation. */
/** @param {string} base @param {(url: string) => Promise<import("../../../types/search").PagefindModule>} [importAPI] */
function createSession(base, importAPI = url => import(url)) {
  /** @type {Map<string, ReturnType<typeof createIndex>>} */
  const versions = new Map();
  /** @type {ReturnType<typeof createIndex> | undefined} */
  let current;
  /** @type {Promise<ReturnType<typeof createIndex>> | undefined} */
  let pending;
  let generation = 0;
  let online = navigator.onLine;
  let stale = false;
  const lifetime = new AbortController();

  /** @param {boolean} [refresh] @returns {Promise<ReturnType<typeof createIndex>>} */
  function load(refresh = false) {
    lifetime.signal.throwIfAborted();
    if (pending) return pending;
    if (current && !refresh && !stale) return Promise.resolve(current);
    const token = generation;
    pending = Promise.resolve().then(async () => {
      try {
        const offline = window.AGGROffline?.search;
        if (!online && (!offline?.activeVersion || !offline.base))
          throw new Error("A complete offline search index is not saved.");
        const catalogueBase = online ? base : new URL(offline?.base || "", base).href;
        const response = await fetch(new URL("search-catalog.json", catalogueBase).href, { cache: "no-store", signal: lifetime.signal });
        if (!response.ok) throw new Error("Search catalogue unavailable.");
        const manifest = /** @type {Catalog} */ (await response.json());
        lifetime.signal.throwIfAborted();
        if (token !== generation) return load();
        const bundle = new URL(manifest.base, base), site = new URL(base);
        if (bundle.origin !== site.origin || !bundle.pathname.startsWith(site.pathname) ||
            typeof manifest.version !== "string" || !manifest.facets || (!online && manifest.version !== offline?.activeVersion))
          throw new Error("Invalid search catalogue.");
        const key = JSON.stringify([manifest.version, bundle.href]);
        current = versions.get(key) || createIndex(base, manifest, importAPI, lifetime.signal);
        versions.delete(key); versions.set(key, current); stale = false;
        if (versions.size > 2) {
          const oldest = versions.keys().next().value;
          if (oldest !== undefined) { versions.get(oldest)?.retire(); versions.delete(oldest); }
        }
        return current;
      } catch (error) {
        if (token === generation && current && !lifetime.signal.aborted) return current;
        throw error;
      } finally { if (token === generation) pending = undefined; }
    });
    return pending;
  }
  return {
    load,
    stale() { generation++; pending = undefined; stale = true; },
    network() {
      if (online === navigator.onLine) return false;
      online = navigator.onLine; generation++; pending = undefined; current = undefined; stale = true;
      return true;
    },
    dispose() {
      lifetime.abort(); generation++; pending = undefined; current = undefined;
      for (const index of versions.values()) index.retire();
      versions.clear();
    },
  };
}

/** @type {Map<string, ReturnType<typeof createSession>>} */
const sessions = new Map();
/** @param {string} base */
function sessionFor(base) {
  let session = sessions.get(base);
  if (!session) { session = createSession(base); sessions.set(base, session); }
  else session.stale();
  session.network();
  return session;
}
window.addEventListener("pagehide", event => {
  if (event.persisted) return;
  for (const session of sessions.values()) session.dispose();
  sessions.clear();
});

/* ------------------------------------------------------------------ controller */

/** @param {import("../../../types/search").MountOptions} options @returns {import("../../../types/search").SearchHandle | undefined} */
export function mount(options) {
  const { base, dates, preferences } = options;
  const root = $("[data-search-root]");
  const inputNode = $("#q");
  const form = $("#search-form");
  const resultsNode = $("[data-search-results]");
  if (!(root instanceof HTMLElement) || !(inputNode instanceof HTMLInputElement) || !form || !resultsNode) return;
  const input = inputNode;
  const results = resultsNode;
  const staticFeed = /** @type {HTMLElement | null} */ ($("[data-static-feed]"));
  const listbox = /** @type {HTMLElement | null} */ ($("#search-completions"));
  const clear = /** @type {HTMLElement | null} */ ($(".search-clear"));
  const status = /** @type {HTMLElement | null} */ ($("#search-status"));
  const list = /** @type {HTMLElement | null} */ ($("#list"));
  const empty = /** @type {HTMLElement | null} */ ($("#empty"));
  const pager = /** @type {HTMLElement | null} */ ($(".search-pager"));
  const error = /** @type {HTMLElement | null} */ ($(".search-error", root));
  const scopeKind = root.dataset.scopeKind;
  const scopeValue = root.dataset.scopeValue;
  const scope = scopeKind && scopeValue ? scopeKind + ":" + quoteValue(scopeValue) : "";
  const engine = sessionFor(base);
  const bindings = new AbortController();
  const signal = bindings.signal;
  /** @type {Catalog | undefined} */
  let catalogue;
  /** @type {Completion[]} */
  let suggestions = [];
  let highlighted = 0;
  let moved = false;
  /** @type {'idle' | 'open' | 'dismissed'} */
  let menu = "idle";
  let composing = false;
  let edited = false;
  let ready = false;
  /** @type {Outcome | undefined} */
  let lastOutcome;
  /** @type {Set<string>} */
  const cachedPreviews = new Set();
  let offlineVersion = window.AGGROffline?.search?.activeVersion;
  let generation = 0;
  let catalogueGeneration = 0;
  let page = 1;
  /** @type {ReturnType<typeof setTimeout> | undefined} */
  let debounce;
  /** @type {AbortController | undefined} */
  let queryWork;
  /** @type {AbortController | undefined} */
  let contextWork;
  /** @type {{key: string, field: string, values: Facet[]} | undefined} */
  let context;

  const pageSize = () => Number(preferences?.values["feed-page-size"]) || 50;
  const active = () => input.value.trim() !== "" && input.value.trim() !== scope;
  /** @param {string} message */
  function showError(message) { if (error) { error.textContent = message; error.hidden = !message; } }
  function hideMenu() {
    if (listbox) { listbox.hidden = true; listbox.replaceChildren(); }
    input.setAttribute("aria-expanded", "false"); input.removeAttribute("aria-activedescendant");
  }
  function closeMenu() { menu = "dismissed"; contextWork?.abort(); hideMenu(); }
  function renderMenu() {
    if (!listbox || signal.aborted) return;
    if (menu !== "open" || !suggestions.length) return hideMenu();
    listbox.replaceChildren(...suggestions.map((suggestion, index) => {
      const detail = suggestion.detail + (suggestion.count !== undefined ? " · " + suggestion.count : "");
      const item = element("li", {
        class: "search-completion", role: "option", id: "search-completion-" + index,
        "data-completion-id": suggestion.id, "aria-selected": String(index === highlighted),
        ...(index === highlighted ? { "data-selected": "" } : {}),
      }, [element("span", { class: "completion-label", text: suggestion.label }),
        detail.trim() ? element("small", { text: detail.trim() }) : null]);
      item.addEventListener("pointerdown", event => { event.preventDefault(); choose(suggestion); });
      item.addEventListener("pointermove", () => {
        if (highlighted === index) return;
        highlighted = index; moved = true; renderMenu();
      });
      return item;
    }));
    listbox.hidden = false;
    input.setAttribute("aria-expanded", "true");
    input.setAttribute("aria-activedescendant", "search-completion-" + highlighted);
    listbox.children[highlighted]?.scrollIntoView({ block: "nearest" });
  }
  function editedFacet() {
    const cursor = input.selectionStart ?? input.value.length;
    const { start, end, value } = completionToken(input.value, cursor);
    const operator = value.match(/^(source|category|tag|type|date|before|after|since|until):/i)?.[1]?.toLowerCase();
    if (!operator) return;
    const field = DATE_FIELDS.includes(operator) ? "published-day" : operator;
    return { field, rest: (input.value.slice(0, start) + input.value.slice(end)).trim() };
  }
  function suggest() {
    if (signal.aborted || composing) return;
    let facets = catalogue?.facets;
    let scoped = false;
    try {
      const edited = editedFacet();
      if (catalogue && edited && edited.rest && menu === "open") {
        const query = parseQuery(edited.rest);
        if (query.clauses.some(clause => clause.kind !== "sort")) {
          scoped = true;
          const key = JSON.stringify([catalogue.version, edited.field, edited.rest]);
          if (context?.key !== key) {
            contextWork?.abort(); contextWork = new AbortController();
            const request = contextWork;
            context = { key, field: edited.field, values: [] };
            void engine.load().then(index => index.counts(query, edited.field, request.signal)).then(values => {
              if (signal.aborted || request.signal.aborted || context?.key !== key) return;
              context.values = values; suggest();
            }).catch(failure => {
              if (!signal.aborted && !request.signal.aborted) showError(failure instanceof Error ? failure.message : "Search suggestions are unavailable.");
            });
          }
          facets = { ...catalogue.facets, [edited.field]: context.values };
        }
      }
      if (!scoped) { contextWork?.abort(); context = undefined; }
      const selected = moved ? suggestions[highlighted]?.id : undefined;
      suggestions = complete(input.value, input.selectionStart ?? input.value.length, facets, Date.now(), catalogue?.facets, scoped);
      highlighted = selected ? Math.max(0, suggestions.findIndex(option => option.id === selected)) : 0;
      renderMenu();
    } catch { suggestions = []; contextWork?.abort(); context = undefined; hideMenu(); }
  }
  function completing() {
    if (!catalogue) return false;
    const { token, value } = completionToken(input.value, input.selectionStart ?? input.value.length);
    const match = value.match(/^(source|category|tag|type):(.*)$/i);
    if (!token || !match) return false;
    // Other invalid clauses must still report their own error.
    parseQuery(input.value.slice(0, token.start) + input.value.slice(token.end));
    const values = catalogue.facets[match[1].toLowerCase()] || [];
    if (validToken(token.raw, Date.now()) && cachedIndex(values, match[1]).lookup(match[2])) return false;
    const candidate = complete(input.value, input.selectionStart ?? input.value.length, catalogue.facets)[0];
    if (!candidate) return false;
    try { parseQuery(acceptCompletion(input.value, candidate).query); return true; }
    catch { return false; }
  }
  /** @param {Completion} suggestion */
  function choose(suggestion) {
    if (signal.aborted || menu !== "open" || !suggestions.some(option => option.id === suggestion.id && option.insert === suggestion.insert && option.start === suggestion.start && option.end === suggestion.end)) return;
    const accepted = acceptCompletion(input.value, suggestion);
    input.value = accepted.query; input.setSelectionRange(accepted.cursor, accepted.cursor);
    changed(0);
    if (!suggestion.insert.endsWith(":")) closeMenu();
  }
  function syncLocation() {
    if (signal.aborted) return;
    try { history.replaceState(history.state, "", active() ? queryURL(location.href, input.value, page) : stripSearch()); }
    catch { /* Incomplete queries are not shareable yet. */ }
  }
  function stripSearch() {
    const url = new URL(location.href); url.searchParams.delete("q"); url.searchParams.delete("search-page");
    return url.href;
  }
  function hideResults() {
    ready = false;
    if (list) { list.hidden = true; list.replaceChildren(); }
    if (empty) empty.hidden = true;
    if (pager) pager.hidden = true;
  }
  function showResultsShell() {
    document.body.setAttribute("data-searching", "");
    if (staticFeed) staticFeed.hidden = true;
  }
  function cancelQuery() { clearTimeout(debounce); generation++; queryWork?.abort(); }
  function reset() {
    cancelQuery(); contextWork?.abort(); context = undefined; page = 1; closeMenu(); hideResults();
    document.body.removeAttribute("data-searching");
    if (staticFeed) staticFeed.hidden = false;
    if (status) status.hidden = true;
    showError(""); syncLocation(); options.selection?.restore();
  }
  /** @param {boolean} [refresh] */
  async function loadFacets(refresh = false) {
    const token = ++catalogueGeneration;
    const index = await engine.load(refresh);
    if (signal.aborted || token !== catalogueGeneration) return;
    catalogue = index.manifest;
    if (!edited && active()) input.value = readableQuery(input.value, catalogue.facets);
    suggest();
    return index;
  }
  /** @param {boolean} [keepRows] @param {boolean} [refresh] */
  async function run(keepRows = false, refresh = false) {
    if (signal.aborted || composing) return;
    cancelQuery(); const token = generation; queryWork = new AbortController(); const work = queryWork;
    if (!active()) return reset();
    showResultsShell(); if (!keepRows) hideResults(); showError("");
    if (status && !ready) { status.hidden = false; status.textContent = "Searching…"; }
    const focused = results.contains(document.activeElement) && document.activeElement instanceof HTMLAnchorElement ? document.activeElement.href : "";
    try {
      const index = await loadFacets(refresh);
      if (!index || signal.aborted || token !== generation) return;
      if (completing()) { if (status) status.hidden = true; return; }
      const query = parseQuery(input.value);
      if (scopeKind && scopeValue && !query.clauses.some(clause => clause.kind === "facet" && clause.field === scopeKind && !clause.exclude && resolveFacet(clause.value, index.manifest.facets[scopeKind] || [], scopeKind).value === scopeValue)) {
        (options.navigate || (href => location.assign(href)))(queryURL(base, input.value, page)); return;
      }
      const outcome = await index.run(query, page, pageSize(), work.signal);
      if (signal.aborted || token !== generation) return;
      page = outcome.page; syncLocation(); await verifyPreviews(outcome);
      if (signal.aborted || token !== generation) return;
      renderResults(outcome); remember(outcome);
      if (focused) list?.querySelectorAll('a').forEach(link => { if (link.href === focused) link.focus({ preventScroll: true }); });
    } catch (failure) {
      if (signal.aborted || token !== generation) return;
      hideResults(); if (status) status.hidden = true;
      showError(failure instanceof QueryError ? failure.message : navigator.onLine ? "Search is unavailable right now. Try again in a moment." : "Offline search is unavailable until the complete index is saved.");
    }
  }
  const SNAPSHOT_KEY = "aggr:search-snapshot:" + encodeURIComponent(new URL(base).pathname);
  /** @param {Outcome} outcome */
  function remember(outcome) {
    const saved = JSON.stringify({ query: input.value, page: outcome.page, outcome });
    if (saved.length <= 512 * 1024) try { sessionStorage.setItem(SNAPSHOT_KEY, saved); } catch { /* Storage is optional. */ }
  }
  function restore() {
    try {
      const saved = JSON.parse(sessionStorage.getItem(SNAPSHOT_KEY) || "null");
      if (!saved || saved.query !== input.value || saved.page !== page || !Array.isArray(saved.outcome?.results)) return false;
      showResultsShell(); renderResults(saved.outcome); return true;
    } catch { return false; }
  }
  /** @param {Outcome} outcome */
  async function verifyPreviews(outcome) {
    if (navigator.onLine || !("caches" in window)) return;
    await Promise.all(outcome.results.map(async result => {
      const preview = displayData(result).preview;
      if (!preview) return;
      const url = safeURL(preview.url, base);
      try {
        const response = await caches.match(url);
        if (response?.ok && response.headers.get("content-type")?.startsWith("image/")) cachedPreviews.add(url);
      } catch { /* Keep unavailable previews absent while offline. */ }
    }));
  }
  /** @param {Outcome} outcome */
  function renderResults(outcome) {
    if (!list || signal.aborted) return;
    lastOutcome = outcome;
    list.replaceChildren(...outcome.results.map(result => renderRow(result, { base, dates, cachedPreviews })));
    const widest = (outcome.page - 1) * outcome.size + outcome.results.length;
    const feedRows = staticFeed?.querySelector('.rows');
    const feed = parseInt(feedRows instanceof HTMLElement ? feedRows.style.getPropertyValue("--rank-indent") : "", 10) || 0;
    list.style.setProperty("--rank-indent", Math.max(feed, String(widest).length - 1) + "ch");
    list.style.setProperty("counter-reset", "rank " + (outcome.page - 1) * outcome.size);
    list.hidden = false; ready = true;
    if (status) { status.hidden = false; status.textContent = outcome.total + (outcome.total === 1 ? " article" : " articles"); }
    if (empty) empty.hidden = outcome.total !== 0;
    if (pager) {
      pager.hidden = outcome.pages <= 1;
      const previous = $("[data-search-page-previous]", pager), next = $("[data-search-page-next]", pager);
      const label = $("[data-search-page-status]", pager);
      if (previous instanceof HTMLAnchorElement) { previous.hidden = outcome.page <= 1; previous.href = queryURL(location.href, input.value, outcome.page - 1); }
      if (next instanceof HTMLAnchorElement) { next.hidden = outcome.page >= outcome.pages; next.href = queryURL(location.href, input.value, outcome.page + 1); }
      if (label) label.textContent = "page " + outcome.page + " / " + outcome.pages;
    }
    options.selection?.restore(); dates.render(list);
    document.dispatchEvent(new CustomEvent("aggr:search-results", { detail: { root: list } }));
  }
  function changed(delay = DEBOUNCE) {
    cancelQuery(); edited = true; page = 1; menu = "open"; moved = false;
    if (clear) clear.hidden = !input.value;
    if (!input.value.trim() && !composing) return reset();
    showResultsShell(); hideResults(); showError(""); suggest();
    if (composing) return;
    const token = generation; queryWork = new AbortController(); const work = queryWork;
    void loadFacets().then(index => {
      if (!index || token !== generation || work.signal.aborted || signal.aborted || completing()) return;
      return index.prepare(parseQuery(input.value), work.signal);
    }).catch(() => {});
    debounce = setTimeout(() => void run(), delay);
  }
  input.addEventListener("input", () => changed(), { signal });
  input.addEventListener("compositionstart", () => { composing = true; cancelQuery(); hideResults(); }, { signal });
  input.addEventListener("compositionend", () => { composing = false; changed(); }, { signal });
  input.addEventListener("focus", () => {
    if (clear) clear.hidden = !input.value;
    if (menu === "idle") menu = "open";
    suggest();
  }, { signal });
  input.addEventListener("blur", () => setTimeout(() => {
    if (!signal.aborted && document.activeElement !== input) { menu = "idle"; hideMenu(); }
  }, 120), { signal });
  input.addEventListener("keyup", event => { if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) suggest(); }, { signal });
  input.addEventListener("click", suggest, { signal });
  input.addEventListener("keydown", event => {
    if (composing || event.isComposing) { event.stopPropagation(); return; }
    const suggesting = menu === "open" && suggestions.length > 0;
    if (event.key === "Escape") {
      event.preventDefault(); event.stopPropagation(); if (suggesting) closeMenu(); else input.blur(); return;
    }
    if ((event.key === "Enter" || event.key === "Tab" && !event.shiftKey) && suggesting) {
      event.preventDefault(); event.stopPropagation(); choose(suggestions[highlighted] || suggestions[0]); return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      if (suggesting) {
        event.preventDefault(); event.stopPropagation(); moved = true;
        highlighted = (highlighted + (event.key === "ArrowDown" ? 1 : -1) + suggestions.length) % suggestions.length;
        renderMenu();
      } else if (ready && options.selection?.move(event.key === "ArrowDown" ? 1 : -1, false)) {
        event.preventDefault(); event.stopPropagation();
      }
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault(); event.stopPropagation(); closeMenu();
      const link = ready ? options.selection?.link(options.selection.selected()) : null;
      if (link) link.click(); else void run();
    }
  }, { signal });
  clear?.addEventListener("click", () => {
    if (scope) { (options.navigate || (href => location.assign(href)))(base); return; }
    input.value = ""; edited = true; clear.hidden = true; reset(); input.focus({ preventScroll: true });
  }, { signal });
  form.addEventListener("submit", event => { event.preventDefault(); closeMenu(); void run(); }, { signal });
  pager?.addEventListener("click", event => {
    if (!(event.target instanceof Element)) return;
    const link = event.target.closest("[data-search-page-previous], [data-search-page-next]");
    if (!link) return;
    event.preventDefault(); page += link.hasAttribute("data-search-page-next") ? 1 : -1;
    void run(); results.scrollIntoView({ block: "start", behavior: "instant" });
  }, { signal });
  window.addEventListener("popstate", () => {
    const url = new URL(location.href); input.value = url.searchParams.get("q") || scope;
    page = Number(url.searchParams.get("search-page")) || 1; closeMenu();
    if (active()) { const restored = restore(); void run(restored); } else reset();
  }, { signal });
  function refresh() {
    if (signal.aborted) return;
    contextWork?.abort(); context = undefined; catalogueGeneration++; engine.stale();
    if (active() && !composing) void run(ready, true);
    else void loadFacets(true).catch(() => {});
  }
  function updateOfflineStatus() { if (engine.network()) refresh(); }
  window.addEventListener("online", updateOfflineStatus, { signal });
  window.addEventListener("offline", updateOfflineStatus, { signal });
  document.addEventListener("aggr:offline-status", () => {
    const version = window.AGGROffline?.search?.activeVersion;
    if (offlineVersion !== version) { offlineVersion = version; if (!navigator.onLine) refresh(); }
    if (lastOutcome && ready) {
      for (const row of list?.querySelectorAll(".row") || []) {
        row.querySelector(".search-saved-status")?.remove();
        if (row instanceof HTMLElement && window.AGGROffline?.saved.some(saved => safeURL(saved.url, base) === row.dataset.url))
          row.querySelector(".row-copy")?.appendChild(element("div", { class: "search-saved-status", "data-saved-offline": "true", text: "Saved offline" }));
      }
    }
  }, { signal });
  function destroy() {
    if (signal.aborted) return;
    cancelQuery(); contextWork?.abort(); catalogueGeneration++; bindings.abort();
  }
  options.signal?.addEventListener("abort", destroy, { once: true });
  if (options.signal?.aborted) { destroy(); return; }
  const url = new URL(location.href);
  if (url.searchParams.has("q")) { input.value = url.searchParams.get("q") || ""; page = Number(url.searchParams.get("search-page")) || 1; }
  if (clear) clear.hidden = !input.value;
  if (document.activeElement === input) menu = "open";
  if (active()) { const restored = restore(); void run(restored); }
  else void loadFacets().catch(() => {});
  return { refresh, updateOfflineStatus, destroy };
}

export { parseQuery, queryURL, complete, acceptCompletion, createSession };
