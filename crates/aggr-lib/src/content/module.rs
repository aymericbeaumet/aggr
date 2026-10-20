//! Articles a page ships only inside a compiled JavaScript module. A Vite or Next build of an
//! MDX post serves an empty shell (`<div id="root">` and one module script) and puts the post
//! itself in that module as `jsx(tag, {children: …})` calls over string literals. Nothing runs:
//! the module is read as text, the static `jsx` call tree is rebuilt, and a bounded whitelist
//! of HTML elements is written back. Anything dynamic in the tree is left out.

use url::Url;

use super::escape_html;
use super::scan::{attribute_value, parse_tag};

/// Module scripts fetched for one shell page. Beyond the entry and its preloads nothing else
/// is worth a request.
const MAX_MODULES: usize = 4;
/// Words a recovered tree must carry to count as the article rather than a widget.
const MIN_ARTICLE_WORDS: usize = 80;
const MIN_ARTICLE_BLOCKS: usize = 3;
/// A shell page shows at most this many words of its own (chrome and a loading hint).
const MAX_SHELL_WORDS: usize = 20;

/// Whether the page is a script shell: next to nothing to read, and at least one module script.
pub fn is_script_shell(page: &str) -> bool {
    let words = visible_words(page);
    (words <= MAX_SHELL_WORDS && page.contains("type=\"module\""))
        || (words == 0 && page.contains("<script"))
}

fn visible_words(page: &str) -> usize {
    let document = scraper::Html::parse_document(page);
    let mut words = 0;
    for node in document.tree.nodes() {
        let Some(text) = node.value().as_text() else {
            continue;
        };
        let inert = node.ancestors().any(|ancestor| {
            ancestor.value().as_element().is_some_and(|element| {
                matches!(
                    element.name(),
                    "script"
                        | "style"
                        | "noscript"
                        | "template"
                        | "head"
                        | "header"
                        | "nav"
                        | "footer"
                )
            })
        });
        if !inert {
            words += text.split_whitespace().count();
        }
    }
    words
}

/// The page's own module scripts and module preloads, same origin only, entry first.
pub fn module_scripts(page: &str, base: &Url) -> Vec<Url> {
    let mut urls: Vec<Url> = Vec::new();
    let mut position = 0;
    while let Some(start) = page[position..].find('<').map(|offset| position + offset) {
        let Some(tag) = parse_tag(&page[start..]) else {
            position = start + 1;
            continue;
        };
        let Some(end) = tag.end else {
            break;
        };
        let raw = &page[start..start + end];
        let candidate = match tag.name.as_str() {
            "script" if !tag.closing && attribute_value(raw, "type") == Some("module") => {
                attribute_value(raw, "src")
            }
            "link" if attribute_value(raw, "rel") == Some("modulepreload") => {
                attribute_value(raw, "href")
            }
            _ => None,
        };
        if let Some(url) = candidate.and_then(|value| base.join(value.trim()).ok())
            && matches!(url.scheme(), "http" | "https")
            && url.origin() == base.origin()
            && !urls.contains(&url)
        {
            urls.push(url);
        }
        position = start + end;
    }
    urls.truncate(MAX_MODULES);
    urls
}

/// The article a module carries, as HTML, when the module carries exactly one. `slug` is the
/// page's own path segment: a bundle holding several posts must name it near the right one.
pub fn article_from_module(source: &str, slug: &str) -> Option<String> {
    let roots = Parser::roots(source);
    let mut candidates: Vec<(usize, &Node)> = roots
        .iter()
        .filter(|(_, node)| {
            node.words() >= MIN_ARTICLE_WORDS && node.blocks() >= MIN_ARTICLE_BLOCKS
        })
        .map(|(start, node)| (*start, node))
        .collect();
    if candidates.len() > 1 {
        // Several posts in one bundle: the one whose metadata (`slug: "…"`) sits nearest wins,
        // and only when that is unambiguous.
        if slug.is_empty() {
            return None;
        }
        let distances: Vec<Option<usize>> = candidates
            .iter()
            .map(|(start, node)| slug_distance(source, *start, node, slug))
            .collect();
        let best = distances.iter().flatten().min().copied()?;
        let nearest: Vec<usize> = distances
            .iter()
            .enumerate()
            .filter(|(_, distance)| **distance == Some(best))
            .map(|(index, _)| index)
            .collect();
        let [index] = nearest.as_slice() else {
            return None;
        };
        return Some(candidates[*index].1.html());
    }
    candidates.pop().map(|(_, node)| node.html())
}

/// How far the nearest mention of `slug` is from the tree, in bytes, looking a bounded distance
/// before and after it; the tree's own text counts as zero.
fn slug_distance(source: &str, start: usize, node: &Node, slug: &str) -> Option<usize> {
    const REACH: usize = 4_000;
    if node.text().contains(slug) {
        return Some(0);
    }
    let end = start + node.html().len().min(source.len() - start);
    let before = &source[start.saturating_sub(REACH)..start];
    let after = &source[end..source.len().min(end + REACH)];
    let backwards = before.rfind(slug).map(|at| before.len() - at);
    let forwards = after.find(slug);
    match (backwards, forwards) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Text(String),
    Element {
        tag: String,
        attributes: Vec<(String, String)>,
        children: Vec<Node>,
    },
}

impl Node {
    fn text(&self) -> String {
        let mut out = String::new();
        self.collect_text(&mut out);
        out
    }

    fn collect_text(&self, out: &mut String) {
        match self {
            Node::Text(text) => {
                out.push_str(text);
                out.push(' ');
            }
            Node::Element { children, .. } => {
                children.iter().for_each(|child| child.collect_text(out))
            }
        }
    }

    fn words(&self) -> usize {
        self.text().split_whitespace().count()
    }

    fn blocks(&self) -> usize {
        match self {
            Node::Text(_) => 0,
            Node::Element { tag, children, .. } => {
                let own = usize::from(matches!(
                    tag.as_str(),
                    "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "pre" | "blockquote"
                ));
                own + children.iter().map(Node::blocks).sum::<usize>()
            }
        }
    }

    fn html(&self) -> String {
        let mut out = String::new();
        self.write_html(&mut out);
        out
    }

    fn write_html(&self, out: &mut String) {
        match self {
            Node::Text(text) => out.push_str(&escape_html(text)),
            Node::Element {
                tag,
                attributes,
                children,
            } => {
                let known = KEPT_ELEMENTS.contains(&tag.as_str());
                if known {
                    out.push('<');
                    out.push_str(tag);
                    for (name, value) in attributes {
                        let kept = matches!(
                            (tag.as_str(), name.as_str()),
                            ("a", "href") | ("img", "src" | "alt") | ("code", "class")
                        );
                        if kept {
                            out.push(' ');
                            out.push_str(name);
                            out.push_str("=\"");
                            out.push_str(&escape_html(value));
                            out.push('"');
                        }
                    }
                    out.push('>');
                }
                if !matches!(tag.as_str(), "img" | "br" | "hr") {
                    children.iter().for_each(|child| child.write_html(out));
                    if known {
                        out.push_str("</");
                        out.push_str(tag);
                        out.push('>');
                    }
                }
            }
        }
    }
}

const KEPT_ELEMENTS: &[&str] = &[
    "p",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "ul",
    "ol",
    "li",
    "blockquote",
    "pre",
    "code",
    "strong",
    "b",
    "em",
    "i",
    "a",
    "img",
    "br",
    "hr",
    "table",
    "thead",
    "tbody",
    "tr",
    "th",
    "td",
    "figure",
    "figcaption",
    "sup",
    "sub",
    "del",
    "s",
    "span",
    "div",
    "section",
];

/// Attributes worth keeping and the children of one `jsx` call.
type Props = (Vec<(String, String)>, Vec<Node>);

enum Value {
    Text(String),
    List(Vec<Value>),
    Node(Node),
    Other,
}

struct Parser<'a> {
    source: &'a str,
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Parser<'a> {
    /// Every outermost static `jsx` call tree in the module, with the offset it starts at.
    fn roots(source: &'a str) -> Vec<(usize, Node)> {
        let mut parser = Parser {
            source,
            bytes: source.as_bytes(),
            position: 0,
        };
        let mut roots = Vec::new();
        while let Some(open) = parser.next_call_site() {
            let start = parser.position;
            parser.position = open;
            match parser.call() {
                Some(node) => roots.push((start, node)),
                None => parser.position = open + 1,
            }
        }
        roots
    }

    /// Offset of the `(` opening the next `jsx`, `jsxs`, `jsxDEV` or `createElement` call at or
    /// after the current position; the position is left at the call's first byte.
    fn next_call_site(&mut self) -> Option<usize> {
        loop {
            let rest = &self.source[self.position..];
            let jsx = rest.find("jsx");
            let create = rest.find("createElement(");
            let (offset, name) = match (jsx, create) {
                (Some(a), Some(b)) if b < a => (b, "createElement("),
                (Some(a), _) => (a, "jsx"),
                (None, Some(b)) => (b, "createElement("),
                (None, None) => return None,
            };
            let at = self.position + offset;
            let before = self.bytes.get(at.wrapping_sub(1)).copied();
            let mut end = at + name.len();
            if name == "jsx" {
                if rest[offset + 3..].starts_with('s') {
                    end += 1;
                } else if rest[offset + 3..].starts_with("DEV") {
                    end += 3;
                }
                if self.bytes.get(end) == Some(&b')') {
                    end += 1;
                }
                if self.bytes.get(end) != Some(&b'(') {
                    self.position = at + 3;
                    continue;
                }
            } else {
                end -= 1;
            }
            // `.jsx(`, `(0,u.jsx)(`, `jsx(`: an identifier boundary, not part of another word.
            if before
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$')
            {
                self.position = at + 3;
                continue;
            }
            // Walk back over `(0,ident.` so the call's start offset covers its whole form.
            let mut start = at;
            if before == Some(b'.') {
                start -= 1;
                while start > 0
                    && (self.bytes[start - 1].is_ascii_alphanumeric()
                        || matches!(self.bytes[start - 1], b'_' | b'$' | b'.'))
                {
                    start -= 1;
                }
                if self.source[..start].ends_with("(0,") {
                    start -= 3;
                }
            }
            self.position = start;
            return Some(end);
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.position += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.skip_ws();
        if self.peek() == Some(byte) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    /// A call whose `(` is at the current position: `(type, props, …)`.
    fn call(&mut self) -> Option<Node> {
        if !self.eat(b'(') {
            return None;
        }
        let tag = self.tag_name()?;
        if !self.eat(b',') {
            return None;
        }
        let (attributes, children) = self.props()?;
        // Trailing arguments (`key`, dev-mode source positions) carry nothing.
        self.skip_balanced_until(b')')?;
        self.position += 1;
        Some(Node::Element {
            tag,
            attributes,
            children,
        })
    }

    /// `t.p`, `"p"`, `` `p` ``, `u.Fragment`, or a component identifier, which renders as its
    /// children.
    fn tag_name(&mut self) -> Option<String> {
        self.skip_ws();
        match self.peek()? {
            b'"' | b'\'' | b'`' => match self.string()? {
                Value::Text(name) => Some(name.to_ascii_lowercase()),
                _ => None,
            },
            byte if byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$' => {
                let start = self.position;
                while self.peek().is_some_and(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'.')
                }) {
                    self.position += 1;
                }
                let name = &self.source[start..self.position];
                let last = name.rsplit('.').next().unwrap_or(name);
                // A member of the components object (`t.p`) names an element; a bare component
                // (`d`, `Fragment`) is transparent.
                Some(
                    if name.contains('.')
                        && last
                            .chars()
                            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
                    {
                        last.to_string()
                    } else {
                        "fragment".to_string()
                    },
                )
            }
            _ => None,
        }
    }

    fn props(&mut self) -> Option<Props> {
        self.skip_ws();
        let mut attributes = Vec::new();
        let mut children = Vec::new();
        if self.peek() != Some(b'{') {
            // `null` props: skip the argument.
            self.skip_balanced_until_any(b",)")?;
            return Some((attributes, children));
        }
        self.position += 1;
        loop {
            self.skip_ws();
            match self.peek()? {
                b'}' => {
                    self.position += 1;
                    return Some((attributes, children));
                }
                b',' => {
                    self.position += 1;
                    continue;
                }
                b'.' => {
                    // `...spread`: an expression that is not ours to read.
                    self.skip_balanced_until_any(b",}")?;
                    continue;
                }
                _ => {}
            }
            let key = self.key()?;
            if !self.eat(b':') {
                // Shorthand `{children}`: dynamic.
                self.skip_balanced_until_any(b",}")?;
                continue;
            }
            let value = self.value()?;
            match (key.as_str(), value) {
                ("children", Value::Text(text)) => children.push(Node::Text(text)),
                ("children", Value::Node(node)) => children.push(node),
                ("children", Value::List(items)) => flatten(items, &mut children),
                ("className", Value::Text(class)) => attributes.push(("class".into(), class)),
                (name @ ("href" | "src" | "alt" | "title"), Value::Text(text)) => {
                    attributes.push((name.to_string(), text));
                }
                _ => {}
            }
        }
    }

    fn key(&mut self) -> Option<String> {
        self.skip_ws();
        match self.peek()? {
            b'"' | b'\'' | b'`' => match self.string()? {
                Value::Text(key) => Some(key),
                _ => None,
            },
            b'[' => {
                self.skip_balanced_until(b']')?;
                self.position += 1;
                Some(String::new())
            }
            _ => {
                let start = self.position;
                while self
                    .peek()
                    .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
                {
                    self.position += 1;
                }
                (self.position > start).then(|| self.source[start..self.position].to_string())
            }
        }
    }

    fn value(&mut self) -> Option<Value> {
        self.skip_ws();
        match self.peek()? {
            b'"' | b'\'' | b'`' => self.string(),
            b'[' => {
                self.position += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_ws();
                    match self.peek()? {
                        b']' => {
                            self.position += 1;
                            return Some(Value::List(items));
                        }
                        b',' => {
                            self.position += 1;
                        }
                        _ => items.push(self.value()?),
                    }
                }
            }
            _ => {
                let start = self.position;
                if let Some(open) = self.call_site_here() {
                    let call_start = self.position;
                    self.position = open;
                    if let Some(node) = self.call() {
                        return Some(Value::Node(node));
                    }
                    self.position = call_start;
                }
                self.position = start;
                self.skip_balanced_until_any(b",}])")?;
                Some(Value::Other)
            }
        }
    }

    /// When a `jsx` call starts exactly here, the offset of its `(`.
    fn call_site_here(&mut self) -> Option<usize> {
        let saved = self.position;
        let open = self.next_call_site();
        let found = self.position == saved;
        self.position = saved;
        found.then_some(open?)
    }

    fn string(&mut self) -> Option<Value> {
        let quote = self.peek()?;
        self.position += 1;
        let mut out = String::new();
        let mut dynamic = false;
        loop {
            let byte = self.peek()?;
            self.position += 1;
            match byte {
                b'\\' => {
                    let escaped = self.peek()?;
                    self.position += 1;
                    match escaped {
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'u' => out.push(self.unicode_escape()?),
                        b'x' => {
                            let hex = self.source.get(self.position..self.position + 2)?;
                            self.position += 2;
                            out.push(char::from(u8::from_str_radix(hex, 16).ok()?));
                        }
                        b'\n' => {}
                        other => {
                            let start = self.position - 1;
                            let ch = self.source[start..].chars().next()?;
                            self.position = start + ch.len_utf8();
                            debug_assert!(other == ch as u8 || ch.len_utf8() > 1);
                            out.push(ch);
                        }
                    }
                }
                b'$' if quote == b'`' && self.peek() == Some(b'{') => {
                    // `${expression}`: the text is not static.
                    dynamic = true;
                    self.position += 1;
                    self.skip_balanced_until(b'}')?;
                    self.position += 1;
                }
                byte if byte == quote => {
                    return Some(if dynamic {
                        Value::Other
                    } else {
                        Value::Text(out)
                    });
                }
                _ => {
                    let start = self.position - 1;
                    let ch = self.source[start..].chars().next()?;
                    self.position = start + ch.len_utf8();
                    out.push(ch);
                }
            }
        }
    }

    fn unicode_escape(&mut self) -> Option<char> {
        let code = if self.peek() == Some(b'{') {
            self.position += 1;
            let end = self.source[self.position..].find('}')? + self.position;
            let code = u32::from_str_radix(&self.source[self.position..end], 16).ok()?;
            self.position = end + 1;
            code
        } else {
            let hex = self.source.get(self.position..self.position + 4)?;
            self.position += 4;
            u32::from_str_radix(hex, 16).ok()?
        };
        char::from_u32(code).or(Some('\u{fffd}'))
    }

    /// Advance to the next `closer` at depth zero, honouring nested brackets and strings; the
    /// position is left on the closer.
    fn skip_balanced_until(&mut self, closer: u8) -> Option<()> {
        self.skip_balanced_until_any(&[closer])
    }

    fn skip_balanced_until_any(&mut self, closers: &[u8]) -> Option<()> {
        let mut depth = 0usize;
        loop {
            let byte = self.peek()?;
            if depth == 0 && closers.contains(&byte) {
                return Some(());
            }
            match byte {
                b'"' | b'\'' | b'`' => {
                    self.string()?;
                    continue;
                }
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.checked_sub(1)?,
                _ => {}
            }
            self.position += 1;
        }
    }
}

fn flatten(items: Vec<Value>, children: &mut Vec<Node>) {
    for item in items {
        match item {
            Value::Text(text) => children.push(Node::Text(text)),
            Value::Node(node) => children.push(node),
            Value::List(nested) => flatten(nested, children),
            Value::Other => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODULE: &str = r#"import{_ as e,d as t,f as n,i as r,l as i,r as a,t as o,u as s}from"./src-GO5ZQO2t.js";var c=t(),l=e(n(),1),u=i();function d(e){let t={a:`a`,p:`p`,...e.components};return(0,u.jsxs)(u.Fragment,{children:[(0,u.jsx)(t.p,{children:`As we develop GLM, the model sometimes exhibits capabilities that surprise us, and even unsettle us.`}),`
`,(0,u.jsx)(t.h1,{children:(0,u.jsx)(t.strong,{children:`Driving Optimization with Dense Feedback`})}),`
`,(0,u.jsx)(t.p,{children:(0,u.jsx)(t.img,{src:`https://cdn.example/figure.png`,alt:``})}),`
`,(0,u.jsx)(t.pre,{children:(0,u.jsx)(t.code,{className:`language-Python`,children:`M = tl.dot(M_chunk, M)  # merge
S_next = tl.dot(M, S) + H
`})}),`
`,(0,u.jsxs)(t.p,{children:[`The fixes were merged upstream into Flash Linear Attention. See `,(0,u.jsx)(t.a,{href:`https://github.com/example/pull/1180`,children:`PR #1180`}),` for details, ${"skipped"} and more words that make this a real paragraph of prose about the work.`]}),`
`,(0,u.jsxs)(t.ul,{children:[`
`,(0,u.jsxs)(t.li,{children:[`First `,(0,u.jsx)(t.strong,{children:`bold`}),` item with a "quoted" word & an ampersand.`]}),`
`,(0,u.jsx)(t.li,{children:`Second item with enough words to count as text in the article body of this test.`}),`
`]}),`
`,(0,u.jsx)(t.p,{children:`Of course, we have not yet reached recursive self-improvement; choosing objectives and assessing risk remain human responsibilities, and progress at this boundary will not slow down because we want it to.`})]})}function f(e={}){let{wrapper:t}=e.components||{};return t?(0,u.jsx)(t,{...e,children:(0,u.jsx)(d,{...e})}):d(e)}(0,c.createRoot)(document.getElementById(`root`)).render((0,u.jsx)(l.StrictMode,{children:(0,u.jsx)(a,{date:`2026-09-17`,title:`Toward Recursive Self-Improvement`,subscribeZai:!0,children:(0,u.jsx)(s,{children:(0,u.jsx)(r,{en:f,components:o})})})}));"#;

    #[test]
    fn a_compiled_mdx_module_becomes_its_article() {
        let html = article_from_module(MODULE, "glm-built-its-inference-infrastructure").unwrap();
        assert!(html.starts_with("<p>As we develop GLM"), "{html}");
        assert!(
            html.contains("<h1><strong>Driving Optimization with Dense Feedback</strong></h1>")
        );
        assert!(html.contains("<img src=\"https://cdn.example/figure.png\" alt=\"\">"));
        assert!(html.contains(
            "<pre><code class=\"language-Python\">M = tl.dot(M_chunk, M)  # merge\nS_next"
        ));
        assert!(html.contains("<a href=\"https://github.com/example/pull/1180\">PR #1180</a>"));
        // A `${…}` template is dynamic and stays out; the rest of the paragraph is kept.
        assert!(!html.contains("skipped"));
        assert!(html.contains("<ul>"));
        assert!(html.contains("&quot;quoted&quot; word &amp; an ampersand"));
        assert!(html.ends_with("because we want it to.</p>"), "{html}");
        // The app shell around the article (`StrictMode`, layout components) is not an article.
        assert!(!html.contains("2026-09-17"));
    }

    #[test]
    fn widgets_and_ambiguous_bundles_yield_nothing() {
        assert!(
            article_from_module("export const x = jsx('p', {children: 'short'});", "post")
                .is_none()
        );
        let post = |slug: &str| {
            format!(
                "const meta_{slug} = {{slug: `{slug}`}}; function C_{slug}() {{ return jsxs(Fragment, {{children: [{}]}}); }}",
                (0..6)
                    .map(|index| format!("jsx('p', {{children: `Paragraph number {index} of the {slug} post carries eighteen words of ordinary prose for the test.`}})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let bundle = format!("{}\n{}", post("alpha"), post("beta"));
        let alpha = article_from_module(&bundle, "alpha").unwrap();
        assert!(alpha.contains("of the alpha post") && !alpha.contains("beta"));
        assert!(article_from_module(&bundle, "gamma").is_none());
        assert!(article_from_module(&bundle, "").is_none());
    }

    #[test]
    fn shells_and_their_modules_are_recognized() {
        let base = Url::parse("https://z.ai/blog/post").unwrap();
        let shell = r#"<!doctype html><html><head><meta charset="utf-8"><title>Blog</title><script type="module" crossorigin src="/blog/assets/post-abc.js"></script><link rel="modulepreload" crossorigin href="/blog/assets/src-def.js"><link rel="modulepreload" href="https://cdn.other/x.js"></head><body><div id="root"></div></body></html>"#;
        assert!(is_script_shell(shell));
        assert_eq!(
            module_scripts(shell, &base)
                .iter()
                .map(Url::as_str)
                .collect::<Vec<_>>(),
            [
                "https://z.ai/blog/assets/post-abc.js",
                "https://z.ai/blog/assets/src-def.js"
            ]
        );
        let article = r#"<html><body><script type="module" src="/a.js"></script><article><p>An ordinary server-rendered page with a real paragraph of readable prose, and then another sentence so that the text outside the chrome is longer than any loading hint a shell would show while its scripts fetch the content, which never runs to more than a handful of words.</p></article></body></html>"#;
        assert!(!is_script_shell(article));
        assert!(module_scripts("<html><body></body></html>", &base).is_empty());
    }
}
