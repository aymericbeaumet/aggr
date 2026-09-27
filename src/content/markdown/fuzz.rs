//! Generated documents run through the conversion, so defects surface here rather than in an
//! archived article. Two oracles, neither of which needs a human to look at the output:
//!
//! - Hard invariants every document must satisfy, however broken: the conversion returns, returns
//!   the same thing twice, keeps its internal markers out of the result, and cannot be made to
//!   emit active content.
//! - On a well-formed document, no invented syntax: the generated text carries no Markdown
//!   meaning, so a `*` or `[` in the rendered reading text was written by the conversion itself
//!   and would show up in the prose.
//!
//! Seeds are fixed, so a failure replays exactly. `AGGR_FUZZ=200000 cargo test --bin aggr fuzz`
//! runs a longer campaign; [`ANOMALIES_ENV`] points the same oracle at a real archive.

use super::{super::render::render_markdown, html_to_text, to_markdown};

/// Documents per run. Enough to cover every production of the grammar many times over while
/// keeping the suite fast; raise it with `AGGR_FUZZ` when hunting.
const DOCUMENTS: u64 = 600;
/// Directory of stored `.html` articles to sweep instead of generating documents.
const ANOMALIES_ENV: &str = "AGGR_CORPUS";

/// xorshift64*, so a seed replays its document exactly.
struct Rng(u64);

impl Rng {
    fn seeded(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

/// Text with no Markdown meaning of its own, including the multi-byte characters that a
/// byte-indexed scan could split. Whatever syntax comes out was not quoted from here.
const WORDS: &[&str] = &[
    "alpha",
    "the quick brown",
    "Ünicode",
    "日本語",
    "🚀",
    "naïve",
    "a",
    "Ελληνικά",
    "עברית",
    "x",
    "0042",
];
const URLS: &[&str] = &[
    "https://example.com/one",
    "/relative/two",
    "#fn:1",
    "https://example.com/a?b=c&d=e",
    "https://example.com/img.png",
];
const ATTRS: &[&str] = &[
    "",
    " class=\"x\"",
    " id=\"fn:1\"",
    " title=\"t\"",
    " dir=\"auto\"",
    " role=\"doc-endnotes\"",
    " target=\"_blank\"",
];
/// Markdown syntax that must never survive into what the reader sees.
const SYNTAX: &str = "[]*_`#|~\\";

fn text(rng: &mut Rng) -> String {
    let mut out = String::new();
    for _ in 0..1 + rng.below(4) {
        out.push_str(rng.pick(WORDS));
        out.push(' ');
    }
    out
}

/// What the surrounding element allows, so a well-formed document stays well-formed: a paragraph
/// holds no blocks, and an anchor holds no anchor.
#[derive(Clone, Copy)]
struct Allowed {
    blocks: bool,
    anchors: bool,
}

impl Allowed {
    const ANY: Self = Self {
        blocks: true,
        anchors: true,
    };

    fn inline(self) -> Self {
        Self {
            blocks: false,
            ..self
        }
    }

    fn linked(self) -> Self {
        Self {
            anchors: false,
            ..self
        }
    }
}

fn fragment(rng: &mut Rng, depth: usize, allowed: Allowed) -> String {
    if depth == 0 {
        return text(rng);
    }
    let attr = rng.pick(ATTRS).to_string();
    let url = rng.pick(URLS).to_string();
    let child = |rng: &mut Rng, allowed: Allowed| {
        let mut out = String::new();
        for _ in 0..1 + rng.below(3) {
            let seed = rng.next();
            out.push_str(&fragment(&mut Rng(seed), depth - 1, allowed));
        }
        out
    };
    let mut children = Vec::new();
    for _ in 0..4 {
        children.push(child(rng, allowed));
    }
    let mut prose = Vec::new();
    for _ in 0..2 {
        prose.push(child(rng, allowed.inline()));
    }
    let mut children = children.into_iter();
    let mut prose = prose.into_iter();
    let mut inner = move || children.next().unwrap_or_default();
    let mut text_only = move || prose.next().unwrap_or_default();
    /// The productions that hold no block of their own. Emphasis is not among them: a publisher
    /// nests emphasis in emphasis about as often as it writes a paragraph inside a word, and the
    /// damaged-document run covers what happens when one does.
    const INLINE: [usize; 9] = [3, 14, 15, 16, 17, 18, 20, 22, 24];
    match if allowed.blocks {
        rng.below(25)
    } else {
        INLINE[rng.below(INLINE.len())]
    } {
        0 => format!("<p{attr}>{}</p>", text_only()),
        1 => format!("<div{attr}>{}</div>", inner()),
        2 if allowed.anchors => {
            let inside = child(rng, allowed.linked());
            format!("<a href=\"{url}\"{attr}>{inside}</a>")
        }
        3 => format!("<img src=\"{url}\" alt=\"{}\">", text(rng)),
        4 => format!("<ul{attr}><li>{}</li><li>{}</li></ul>", inner(), inner()),
        5 => format!("<ol{attr}><li id=\"fn:1\">{}</li></ol>", inner()),
        6 => format!("<blockquote{attr}>{}</blockquote>", inner()),
        7 => format!("<pre><code>{}</code></pre>", text(rng)),
        8 => format!("<h{0}{attr}>{1}</h{0}>", 1 + rng.below(6), text_only()),
        9 => format!(
            "<figure{attr}>{}<figcaption>{}</figcaption></figure>",
            inner(),
            text_only()
        ),
        10 => format!(
            "<table><tr><td>{}</td><td>{}</td></tr></table>",
            inner(),
            inner()
        ),
        11 => "<sup><a href=\"#fn:1\">1</a></sup>".to_string(),
        12 => format!("<em>{}</em>", text_only()),
        13 => format!("<strong>{}</strong>", text_only()),
        14 => format!("<span{attr}>{}</span>", text_only()),
        15 => format!("<br>{}", text(rng)),
        16 => {
            format!("<picture><source srcset=\"{url} 2x\"><img src=\"{url}\" alt=\"\"></picture>")
        }
        17 => format!("<iframe src=\"{url}\"></iframe>"),
        18 => format!(
            "<label for=\"fn-1\" data-n=\"1\"></label><span data-n=\"1\">{}</span>",
            text(rng)
        ),
        19 => format!("<section{attr}>{}</section>", inner()),
        20 => format!("<code>{}</code>", text(rng)),
        21 => format!("<!-- {} -->{}", text(rng), inner()),
        22 => format!("<small>{}</small><span>{}</span>", text(rng), text(rng)),
        23 => format!("<hr>{}", inner()),
        _ => text(rng),
    }
}

/// Break the document the way a real page is broken: truncated mid-tag, a closing tag missing or
/// one too many, a span of bytes repeated.
fn damage(rng: &mut Rng, mut html: String) -> String {
    for _ in 0..1 + rng.below(3) {
        if html.is_empty() {
            break;
        }
        let boundary = |html: &str, at: usize| {
            (0..=at.min(html.len()))
                .rev()
                .find(|at| html.is_char_boundary(*at))
                .unwrap_or(0)
        };
        match rng.below(6) {
            0 => {
                let at = boundary(&html, rng.below(html.len()));
                html.truncate(at);
            }
            1 => html = html.replacen("</div>", "", 1),
            2 => html = html.replacen('>', "", 1),
            3 => html.push_str("</div></p></a>"),
            4 => {
                let at = boundary(&html, rng.below(html.len()));
                let repeated = html[at..].to_string();
                html.push_str(&repeated);
            }
            _ => html.insert_str(0, "<a href=\"https://example.com/x\">"),
        }
    }
    html
}

fn document(seed: u64, damaged: bool) -> String {
    let mut rng = Rng::seeded(seed);
    let mut html = String::new();
    for _ in 0..1 + rng.below(4) {
        let depth = 1 + rng.below(4);
        html.push_str(&fragment(&mut rng, depth, Allowed::ANY));
    }
    if damaged {
        damage(&mut rng, html)
    } else {
        html
    }
}

fn base() -> url::Url {
    url::Url::parse("https://example.com/blog/post/").expect("valid base")
}

/// What the conversion must never do, whatever it was handed.
fn invariant_failure(html: &str) -> Option<String> {
    let convert = || to_markdown(html, Some(&base()));
    let markdown = std::panic::catch_unwind(convert).ok()?;
    if markdown != to_markdown(html, Some(&base())) {
        return Some("conversion is not deterministic".into());
    }
    if markdown.contains([
        super::FOOTNOTE_REF_START,
        super::FOOTNOTE_REF_END,
        super::MARKDOWN_LINK_START,
    ]) {
        return Some("an internal marker reached the output".into());
    }
    let rendered = render_markdown(&markdown);
    (rendered.contains("<script") || rendered.contains("javascript:"))
        .then(|| "active content survived".into())
}

/// Drop the formulas the renderer translated: a matrix reads as `[1 0]` and a norm as `‖x‖`, so
/// their brackets and bars are the formula rather than syntax the conversion let slip. TeX it
/// could not translate stays in a `<code>` span and keeps counting, which is the defect to find.
fn without_rendered_math(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find("<span class=\"math") {
        out.push_str(&rest[..at]);
        let Some(close) = rest[at..].find("</span>") else {
            break;
        };
        rest = &rest[at + close + "</span>".len()..];
    }
    out.push_str(rest);
    out
}

/// Markdown syntax in the reading text of a document whose own text had none, with the passage
/// it appears in: a report nobody has to open the article to act on.
pub(super) fn invented_syntax(html: &str, base: Option<&url::Url>) -> Option<(char, String)> {
    let markdown = to_markdown(html, base);
    let visible = html_to_text(&without_rendered_math(&render_markdown(&markdown)));
    let source = html_to_text(html);
    let symbol = SYNTAX
        .chars()
        .find(|symbol| visible.matches(*symbol).count() > source.matches(*symbol).count())?;
    // The last occurrence beyond what the page itself wrote is the one the conversion added.
    let skip = source.matches(symbol).count();
    let at = visible
        .match_indices(symbol)
        .nth(skip)
        .map_or(0, |(at, _)| at);
    let start = visible[..at]
        .char_indices()
        .rev()
        .nth(50)
        .map_or(0, |(index, _)| index);
    Some((symbol, visible[start..].chars().take(110).collect()))
}

fn documents() -> u64 {
    std::env::var("AGGR_FUZZ")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DOCUMENTS)
}

#[test]
fn conversion_holds_its_invariants_on_damaged_documents() {
    for seed in 1..=documents() {
        let html = document(seed, true);
        if let Some(failure) = invariant_failure(&html) {
            panic!("seed {seed}: {failure}\n{html}");
        }
    }
}

/// The shapes a publisher actually writes that the converter used to spell wrong. Each one is a
/// class the corpus sweep or the generator turned up; the text carries no Markdown meaning, so
/// anything in [`SYNTAX`] that reaches the reader was written by the conversion.
#[test]
fn publisher_shapes_never_gain_markdown_syntax() {
    let shapes = [
        // A Substack embed card: one anchor around the whole quoted post.
        (
            "embed card",
            "<p>They wrote:</p><a href=\"https://x.com/a/1\"><div><div><picture><img src=\"https://cdn.test/avatar.jpg\" alt=\"avatar\"></picture></div><div>Name</div></div><div>The post.</div></a>",
        ),
        // An anchor the page never closed still ends somewhere.
        (
            "unclosed anchor",
            "<a href=\"https://example.com/x\">lead <blockquote>quoted</blockquote>",
        ),
        // Emphasis around punctuation, which no pair of asterisks can hold.
        (
            "emphasised comma",
            "<p>context<strong>,</strong>tools<strong>,</strong> more</p>",
        ),
        (
            "emphasised stop",
            "<p>the transcript<em>.</em> For instance</p>",
        ),
        // Emphasis whose closing marker would not flank: the dash moves out instead.
        (
            "emphasis to a dash",
            "<p>a setting <em>with AI—</em>like this</p>",
        ),
        (
            "emphasis to a space",
            "<p>price<strong>. </strong>Fable will</p>",
        ),
        (
            "emphasis over a break",
            "<p><strong><br>gamma delta </strong>tail</p>",
        ),
        (
            "adjacent emphasis",
            "<p><strong>one</strong><strong>two</strong> three</p>",
        ),
        // Inline wrappers cannot hold blocks, and markers with nothing to mark are noise.
        ("emphasised block", "<em><p>gamma</p>tail</em>"),
        ("emphasised heading", "<em><h1>title</h1></em>"),
        ("wrapped heading", "x <span><h4>heading</h4></span> tail"),
        ("empty heading", "x<span><h4></h4></span><br>tail"),
        ("empty listing", "<span><pre></pre></span><p>after</p>"),
        ("adjacent code", "<p><code>one</code><code>two</code></p>"),
        // A bare URL is linkified as written, so an escape inside it is part of the address.
        (
            "escaped url",
            "<p>see https://github.com/run-llama/llama_index/pull/172 now</p>",
        ),
    ];
    for (name, html) in shapes {
        if let Some((symbol, passage)) = invented_syntax(html, Some(&base())) {
            panic!(
                "{name}: the conversion wrote {symbol:?} into {passage:?}\n{}",
                to_markdown(html, Some(&base()))
            );
        }
        assert!(invariant_failure(html).is_none(), "{name}");
    }
}

/// Point the same oracle at a real archive: `AGGR_CORPUS=<store>/items cargo test --bin aggr
/// stored_articles -- --nocapture`. Silent without it, because the corpus is the reader's own and
/// not the repository's. Converting must never fail; the rest is a report of what to look at,
/// since a page is free to print Markdown syntax of its own and some of it is faithfully kept.
#[test]
fn stored_articles_convert_and_report_their_anomalies() {
    let Ok(root) = std::env::var(ANOMALIES_ENV) else {
        return;
    };
    let mut pending = vec![std::path::PathBuf::from(root)];
    let mut articles = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|kind| kind == "html") {
                articles.push(path);
            }
        }
    }
    articles.sort();
    let mut anomalies = Vec::new();
    let mut failures = Vec::new();
    for article in &articles {
        let Ok(html) = std::fs::read_to_string(article) else {
            continue;
        };
        let link = std::fs::read_to_string(article.with_extension("md"))
            .ok()
            .and_then(|front| {
                let line = front.lines().take(40).find(|l| l.starts_with("link: "))?;
                url::Url::parse(line.trim_start_matches("link: ").trim()).ok()
            });
        match std::panic::catch_unwind(|| invented_syntax(&html, link.as_ref())) {
            Ok(Some((symbol, passage))) => anomalies.push(format!(
                "  {symbol:?} {}\n      {}",
                article.display(),
                passage.replace('\n', " ")
            )),
            Ok(None) => {}
            Err(_) => failures.push(article.display().to_string()),
        }
    }
    println!(
        "{} of {} stored articles gained Markdown syntax:\n{}",
        anomalies.len(),
        articles.len(),
        anomalies.join("\n")
    );
    assert!(
        failures.is_empty(),
        "conversion panicked on:\n{failures:#?}"
    );
}
