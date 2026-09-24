//! Dollar-delimited TeX in article bodies (arxiv.org abstracts write `$\log_2 3 \approx 1.585$`)
//! shown as readable text without a formula engine: a bounded translation of the common commands,
//! scripts and delimiters to Unicode. A formula the translator does not fully know is shown as its
//! original TeX in a code span rather than half-translated, and a dollar span that does not read
//! as TeX at all (`$AAPL/$MSFT`) is put back as the prose it was.

use super::escape_html;
use super::strip::decode_entities;

const OPEN_PREFIX: &str = "<span data-math-style=\"";
const INLINE_OPEN: &str = "<span data-math-style=\"inline\">";
const DISPLAY_OPEN: &str = "<span data-math-style=\"display\">";
const CLOSE: &str = "</span>";

/// Replace the math spans comrak emitted for `$…$` and `$$…$$` with their readable rendering.
pub(super) fn render_math_spans(html: &str) -> String {
    if !html.contains(OPEN_PREFIX) {
        return html.to_string();
    }
    let marker = marker_letter(html);
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(OPEN_PREFIX) {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let (display, open) = if after.starts_with(DISPLAY_OPEN) {
            (true, DISPLAY_OPEN.len())
        } else if after.starts_with(INLINE_OPEN) {
            (false, INLINE_OPEN.len())
        } else {
            out.push_str(&after[..1]);
            rest = &after[1..];
            continue;
        };
        // comrak escapes the literal, so the first closing tag is the span's own.
        let Some(end) = after[open..].find(CLOSE) else {
            out.push_str(after);
            return out;
        };
        let literal = unescape_markdown(&decode_entities(&after[open..open + end]));
        let literal = without_marker(&literal, marker);
        out.push_str(&render_formula(literal, display));
        rest = &after[open + end + CLOSE.len()..];
    }
    out.push_str(rest);
    out
}

/// Some publishing pipelines wrap every formula in a marker letter (`$$m … m$$`, `$m … m$`) that
/// their own renderer strips. The display formulas give it away: a lone letter, set apart by
/// whitespace, opening and closing the same formula is not TeX. Once a document shows that, the
/// same letter at both ends of any of its formulas is the marker.
fn marker_letter(html: &str) -> Option<char> {
    let mut rest = html;
    while let Some(start) = rest.find(DISPLAY_OPEN) {
        let after = &rest[start + DISPLAY_OPEN.len()..];
        let end = after.find(CLOSE)?;
        let literal = unescape_markdown(&decode_entities(&after[..end]));
        let tex = literal.trim();
        let first = tex.chars().next()?;
        if first.is_ascii_alphabetic()
            && tex.ends_with(first)
            && tex.chars().count() > 4
            && tex[first.len_utf8()..].starts_with(char::is_whitespace)
            && tex[..tex.len() - first.len_utf8()].ends_with(char::is_whitespace)
        {
            return Some(first);
        }
        rest = &after[end..];
    }
    None
}

fn without_marker(tex: &str, marker: Option<char>) -> &str {
    let Some(marker) = marker else {
        return tex;
    };
    let trimmed = tex.trim();
    match trimmed
        .strip_prefix(marker)
        .and_then(|rest| rest.strip_suffix(marker))
    {
        Some(inner) if trimmed.chars().count() > 2 => inner,
        _ => tex,
    }
}

fn render_formula(tex: &str, display: bool) -> String {
    if !looks_like_tex(tex) {
        let fence = if display { "$$" } else { "$" };
        return escape_html(&format!("{fence}{tex}{fence}"));
    }
    let class = if display { "math math-display" } else { "math" };
    match to_unicode(tex) {
        Some(text) => {
            let separator = if display { "<br>" } else { "; " };
            let rows = text
                .lines()
                .map(escape_html)
                .collect::<Vec<_>>()
                .join(separator);
            format!("<span class=\"{class}\">{rows}</span>")
        }
        None => format!("<code class=\"{class}\">{}</code>", escape_html(tex.trim())),
    }
}

/// The Markdown body escaped punctuation when it was written from HTML (`\\log\_2` for
/// `\log_2`); Markdown leaves a math span's literal untouched, so those escapes are undone here.
fn unescape_markdown(literal: &str) -> String {
    let mut out = String::with_capacity(literal.len());
    let mut chars = literal.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek().is_some_and(char::is_ascii_punctuation) {
            out.push(chars.next().expect("peeked punctuation"));
        } else {
            out.push(ch);
        }
    }
    out
}

/// TeX has commands, scripts, groups or relations; a formula of bare words is prose that happened
/// to sit between two dollar signs.
fn looks_like_tex(tex: &str) -> bool {
    let tex = tex.trim();
    if tex.is_empty() {
        return false;
    }
    if tex.contains(['\\', '^', '_', '{', '}', '=']) {
        return true;
    }
    let mut run = 0;
    let mut longest = 0;
    for ch in tex.chars() {
        run = if ch.is_alphabetic() { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest <= 1
}

fn to_unicode(tex: &str) -> Option<String> {
    let chars: Vec<char> = tex.chars().collect();
    let mut pos = 0;
    let text = sequence(&chars, &mut pos, false)?;
    (pos == chars.len()).then(|| {
        text.lines()
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// `\begin{align*} a &= b \\ c &= d \end{align*}`: an aligned or gathered environment is a
/// stack of rows. Alignment points vanish and each row becomes a line of its own.
fn environment(chars: &[char], pos: &mut usize) -> Option<String> {
    let name = raw_group(chars, pos)?;
    if !matches!(
        name.trim_end_matches('*'),
        "align"
            | "aligned"
            | "alignat"
            | "gather"
            | "gathered"
            | "cases"
            | "split"
            | "multline"
            | "eqnarray"
            | "equation"
            // A matrix reads as its rows; the brackets around them are the shape, not the values.
            | "matrix"
            | "bmatrix"
            | "pmatrix"
            | "vmatrix"
            | "Vmatrix"
            | "Bmatrix"
            | "smallmatrix"
    ) {
        return None;
    }
    let closing: Vec<char> = format!("\\end{{{name}}}").chars().collect();
    let body_start = *pos;
    let body_end = (body_start..chars.len()).find(|&at| chars[at..].starts_with(&closing))?;
    let body = chars[body_start..body_end].to_vec();
    *pos = body_end + closing.len();
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut at = 0;
    while at < body.len() {
        if body[at] == '\\' && body.get(at + 1) == Some(&'\\') {
            rows.push(std::mem::take(&mut row));
            at += 2;
            continue;
        }
        if body[at] == '\\' && body.get(at + 1) == Some(&'&') {
            row.push('&');
            at += 2;
            continue;
        }
        if body[at] != '&' {
            row.push(body[at]);
        }
        at += 1;
    }
    rows.push(row);
    let mut out = String::new();
    for row in rows {
        let mut row_pos = 0;
        let text = sequence(&row, &mut row_pos, false)?;
        if !text.trim().is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text.trim());
        }
    }
    // A matrix keeps the delimiters that say what it is; the rows alone would read as prose.
    let (open, close) = match name.trim_end_matches('*') {
        "bmatrix" => ("[", "]"),
        "pmatrix" => ("(", ")"),
        "vmatrix" => ("|", "|"),
        "Vmatrix" => ("\u{2016}", "\u{2016}"),
        "Bmatrix" => ("{", "}"),
        _ => return Some(out),
    };
    Some(format!("{open}{out}{close}"))
}

/// The literal characters of the `{…}` group at `pos`, for names that must not be translated.
fn raw_group(chars: &[char], pos: &mut usize) -> Option<String> {
    if chars.get(*pos) != Some(&'{') {
        return None;
    }
    let end = (*pos..chars.len()).find(|&at| chars[at] == '}')?;
    let name: String = chars[*pos + 1..end].iter().collect();
    *pos = end + 1;
    Some(name)
}

/// Convert until the end of the input, or until the closing brace of the group being read (which
/// the caller consumes).
fn sequence(chars: &[char], pos: &mut usize, in_group: bool) -> Option<String> {
    let mut out = String::new();
    while let Some(&ch) = chars.get(*pos) {
        match ch {
            '}' => return in_group.then_some(out),
            '{' => out.push_str(&group(chars, pos)?),
            '\\' => {
                *pos += 1;
                out.push_str(&command(chars, pos)?);
            }
            '^' | '_' => {
                *pos += 1;
                let argument = argument(chars, pos)?;
                out.push_str(&script(&argument, ch == '^'));
            }
            _ => {
                *pos += 1;
                out.push_str(&plain(ch));
            }
        }
    }
    (!in_group).then_some(out)
}

fn plain(ch: char) -> String {
    match ch {
        '-' => "\u{2212}".into(),
        '\'' => "\u{2032}".into(),
        '~' => " ".into(),
        ch if ch.is_whitespace() => " ".into(),
        ch => ch.into(),
    }
}

fn group(chars: &[char], pos: &mut usize) -> Option<String> {
    debug_assert_eq!(chars.get(*pos), Some(&'{'));
    *pos += 1;
    let inner = sequence(chars, pos, true)?;
    (chars.get(*pos) == Some(&'}')).then(|| {
        *pos += 1;
        inner
    })
}

/// The operand of a script or a command: a braced group, a command, or one character.
fn argument(chars: &[char], pos: &mut usize) -> Option<String> {
    while chars.get(*pos).is_some_and(|ch| ch.is_whitespace()) {
        *pos += 1;
    }
    match chars.get(*pos)? {
        '{' => group(chars, pos),
        '\\' => {
            *pos += 1;
            command(chars, pos)
        }
        '}' | '^' | '_' => None,
        &ch => {
            *pos += 1;
            Some(plain(ch))
        }
    }
}

fn script(argument: &str, superscript: bool) -> String {
    let table = if superscript {
        SUPERSCRIPTS
    } else {
        SUBSCRIPTS
    };
    let mapped = argument
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .map(|ch| {
            table
                .iter()
                .find(|(from, _)| *from == ch)
                .map(|(_, to)| *to)
        })
        .collect::<Option<String>>();
    match mapped {
        Some(mapped) if !mapped.is_empty() => mapped,
        _ => {
            let marker = if superscript { '^' } else { '_' };
            if argument.chars().count() > 1 {
                format!("{marker}({argument})")
            } else {
                format!("{marker}{argument}")
            }
        }
    }
}

fn command(chars: &[char], pos: &mut usize) -> Option<String> {
    let first = *chars.get(*pos)?;
    if !first.is_ascii_alphabetic() {
        *pos += 1;
        return match first {
            '{' | '}' | '%' | '_' | '$' | '&' | '#' => Some(first.into()),
            ' ' => Some(" ".into()),
            ',' | ';' | ':' | '!' => Some("\u{2009}".into()),
            '\\' => Some(" ".into()),
            '|' => Some("\u{2016}".into()),
            _ => None,
        };
    }
    let start = *pos;
    while chars.get(*pos).is_some_and(char::is_ascii_alphabetic) {
        *pos += 1;
    }
    let name: String = chars[start..*pos].iter().collect();
    match name.as_str() {
        "begin" => environment(chars, pos),
        "frac" | "dfrac" | "tfrac" => {
            let numerator = argument(chars, pos)?;
            let denominator = argument(chars, pos)?;
            Some(format!(
                "{}/{}",
                parenthesized(&numerator),
                parenthesized(&denominator)
            ))
        }
        "sqrt" => {
            if chars.get(*pos) == Some(&'[') {
                return None;
            }
            Some(format!("\u{221a}{}", parenthesized(&argument(chars, pos)?)))
        }
        // `\mathcal{L}` is a letter in another hand; Unicode has that hand.
        "mathcal" | "mathscr" => {
            let inner = argument(chars, pos)?;
            let mut letters = inner.chars();
            let (Some(letter), None) = (letters.next(), letters.next()) else {
                return None;
            };
            SCRIPT
                .iter()
                .find(|(from, _)| *from == letter)
                .map(|(_, to)| (*to).to_string())
        }
        "mathbb" => {
            let inner = argument(chars, pos)?;
            let mut letters = inner.chars();
            let (Some(letter), None) = (letters.next(), letters.next()) else {
                return None;
            };
            BLACKBOARD
                .iter()
                .find(|(from, _)| *from == letter)
                .map(|(_, to)| (*to).to_string())
        }
        // A font is presentation the reader's own type already provides; the argument is the text.
        "mathrm" | "mathit" | "mathbf" | "mathsf" | "mathtt" | "boldsymbol" | "bm" | "text"
        | "textrm" | "textit" | "textbf" | "texttt" | "textsf" | "textnormal" | "emph"
        | "operatorname" => argument(chars, pos),
        "hat" | "bar" | "vec" | "tilde" | "dot" | "ddot" | "overline" | "widehat" | "widetilde"
        | "overrightarrow" | "overleftarrow" => {
            let inner = argument(chars, pos)?;
            let mut letters = inner.chars();
            let (Some(letter), None) = (letters.next(), letters.next()) else {
                return None;
            };
            let accent = match name.as_str() {
                "hat" | "widehat" => '\u{302}',
                "overrightarrow" => '\u{20d7}',
                "overleftarrow" => '\u{20d6}',
                "widetilde" => '\u{303}',
                "bar" | "overline" => '\u{304}',
                "vec" => '\u{20d7}',
                "tilde" => '\u{303}',
                "dot" => '\u{307}',
                _ => '\u{308}',
            };
            Some(format!("{letter}{accent}"))
        }
        "left" | "right" | "big" | "Big" | "bigl" | "bigr" | "Bigl" | "Bigr" | "bigg" | "Bigg"
        | "biggl" | "biggr" | "Biggl" | "Biggr" | "middle" => {
            // The delimiter follows: `\left(`, `\left\{`, or `\left.` for none.
            match chars.get(*pos)? {
                '.' => {
                    *pos += 1;
                    Some(String::new())
                }
                '\\' => {
                    *pos += 1;
                    command(chars, pos)
                }
                &ch => {
                    *pos += 1;
                    Some(ch.into())
                }
            }
        }
        // A box, a rule, a brace or a colour is presentation around the argument.
        "boxed" | "underline" | "underbrace" | "overbrace" | "mathring" => argument(chars, pos),
        // `\color{red}{x}` names the colour first; `\textcolor{red}{x}` too.
        "color" | "textcolor" => {
            let _colour = argument(chars, pos)?;
            Some(argument(chars, pos).unwrap_or_default())
        }
        // `\overset{a}{b}` and `\stackrel{a}{b}` set the first over the second; the second reads.
        "overset" | "stackrel" | "underset" => {
            let _over = argument(chars, pos)?;
            argument(chars, pos)
        }
        "binom" | "dbinom" | "tbinom" => {
            let upper = argument(chars, pos)?;
            let lower = argument(chars, pos)?;
            Some(format!("({upper} choose {lower})"))
        }
        "xrightarrow" | "xleftarrow" => {
            let label = argument(chars, pos)?;
            let arrow = if name == "xrightarrow" { "→" } else { "←" };
            Some(if label.trim().is_empty() {
                arrow.to_string()
            } else {
                format!(" {arrow}[{}] ", label.trim())
            })
        }
        "quad" | "qquad" | "displaystyle" | "textstyle" | "scriptstyle" | "nonumber" | "small"
        | "large" | "Large" | "LARGE" | "scriptsize" | "footnotesize" | "normalsize" | "limits"
        | "nolimits" => Some(" ".into()),
        _ => SYMBOLS
            .iter()
            .find(|(from, _)| *from == name)
            .map(|(_, to)| (*to).to_string())
            .or_else(|| FUNCTIONS.contains(&name.as_str()).then_some(name)),
    }
}

fn parenthesized(text: &str) -> String {
    if text.chars().count() > 1
        && text.contains([
            ' ', '+', '\u{2212}', '\u{b1}', '\u{d7}', '\u{22c5}', '/', '=', '<', '>', '\u{2264}',
            '\u{2265}', '\u{2248}',
        ])
    {
        format!("({text})")
    } else {
        text.to_string()
    }
}

const SUPERSCRIPTS: &[(char, char)] = &[
    ('0', '⁰'),
    ('1', '¹'),
    ('2', '²'),
    ('3', '³'),
    ('4', '⁴'),
    ('5', '⁵'),
    ('6', '⁶'),
    ('7', '⁷'),
    ('8', '⁸'),
    ('9', '⁹'),
    ('+', '⁺'),
    ('\u{2212}', '⁻'),
    ('=', '⁼'),
    ('(', '⁽'),
    (')', '⁾'),
    ('a', 'ᵃ'),
    ('b', 'ᵇ'),
    ('c', 'ᶜ'),
    ('d', 'ᵈ'),
    ('e', 'ᵉ'),
    ('f', 'ᶠ'),
    ('g', 'ᵍ'),
    ('h', 'ʰ'),
    ('i', 'ⁱ'),
    ('j', 'ʲ'),
    ('k', 'ᵏ'),
    ('l', 'ˡ'),
    ('m', 'ᵐ'),
    ('n', 'ⁿ'),
    ('o', 'ᵒ'),
    ('p', 'ᵖ'),
    ('r', 'ʳ'),
    ('s', 'ˢ'),
    ('t', 'ᵗ'),
    ('u', 'ᵘ'),
    ('v', 'ᵛ'),
    ('w', 'ʷ'),
    ('x', 'ˣ'),
    ('y', 'ʸ'),
    ('z', 'ᶻ'),
    ('T', 'ᵀ'),
];

const SUBSCRIPTS: &[(char, char)] = &[
    ('0', '₀'),
    ('1', '₁'),
    ('2', '₂'),
    ('3', '₃'),
    ('4', '₄'),
    ('5', '₅'),
    ('6', '₆'),
    ('7', '₇'),
    ('8', '₈'),
    ('9', '₉'),
    ('+', '₊'),
    ('\u{2212}', '₋'),
    ('=', '₌'),
    ('(', '₍'),
    (')', '₎'),
    ('a', 'ₐ'),
    ('e', 'ₑ'),
    ('h', 'ₕ'),
    ('i', 'ᵢ'),
    ('j', 'ⱼ'),
    ('k', 'ₖ'),
    ('l', 'ₗ'),
    ('m', 'ₘ'),
    ('n', 'ₙ'),
    ('o', 'ₒ'),
    ('p', 'ₚ'),
    ('r', 'ᵣ'),
    ('s', 'ₛ'),
    ('t', 'ₜ'),
    ('u', 'ᵤ'),
    ('v', 'ᵥ'),
    ('x', 'ₓ'),
];

/// `\mathcal` letters, as Unicode writes them. Only the letters a reader meets in prose.
const SCRIPT: &[(char, char)] = &[
    ('A', '𝒜'),
    ('B', 'ℬ'),
    ('C', '𝒞'),
    ('D', '𝒟'),
    ('E', 'ℰ'),
    ('F', 'ℱ'),
    ('G', '𝒢'),
    ('H', 'ℋ'),
    ('I', 'ℐ'),
    ('J', '𝒥'),
    ('K', '𝒦'),
    ('L', 'ℒ'),
    ('M', 'ℳ'),
    ('N', '𝒩'),
    ('O', '𝒪'),
    ('P', '𝒫'),
    ('Q', '𝒬'),
    ('R', 'ℛ'),
    ('S', '𝒮'),
    ('T', '𝒯'),
    ('U', '𝒰'),
    ('V', '𝒱'),
    ('W', '𝒲'),
    ('X', '𝒳'),
    ('Y', '𝒴'),
    ('Z', '𝒵'),
];

const BLACKBOARD: &[(char, char)] = &[
    ('A', '𝔸'),
    ('B', '𝔹'),
    ('C', 'ℂ'),
    ('D', '𝔻'),
    ('E', '𝔼'),
    ('F', '𝔽'),
    ('G', '𝔾'),
    ('H', 'ℍ'),
    ('I', '𝕀'),
    ('J', '𝕁'),
    ('K', '𝕂'),
    ('L', '𝕃'),
    ('M', '𝕄'),
    ('N', 'ℕ'),
    ('O', '𝕆'),
    ('P', 'ℙ'),
    ('Q', 'ℚ'),
    ('R', 'ℝ'),
    ('S', '𝕊'),
    ('T', '𝕋'),
    ('U', '𝕌'),
    ('V', '𝕍'),
    ('W', '𝕎'),
    ('X', '𝕏'),
    ('Y', '𝕐'),
    ('Z', 'ℤ'),
];

const FUNCTIONS: &[&str] = &[
    "log", "ln", "lg", "exp", "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos",
    "arctan", "sinh", "cosh", "tanh", "max", "min", "sup", "inf", "lim", "det", "dim", "gcd",
    "ker", "deg", "arg", "Pr", "argmax", "argmin", "softmax",
];

const SYMBOLS: &[(&str, &str)] = &[
    ("alpha", "α"),
    ("beta", "β"),
    ("gamma", "γ"),
    ("delta", "δ"),
    ("epsilon", "ϵ"),
    ("varepsilon", "ε"),
    ("zeta", "ζ"),
    ("eta", "η"),
    ("theta", "θ"),
    ("vartheta", "ϑ"),
    ("iota", "ι"),
    ("kappa", "κ"),
    ("lambda", "λ"),
    ("mu", "μ"),
    ("nu", "ν"),
    ("xi", "ξ"),
    ("pi", "π"),
    ("rho", "ρ"),
    ("sigma", "σ"),
    ("tau", "τ"),
    ("upsilon", "υ"),
    ("phi", "ϕ"),
    ("varphi", "φ"),
    ("chi", "χ"),
    ("psi", "ψ"),
    ("omega", "ω"),
    ("Gamma", "Γ"),
    ("Delta", "Δ"),
    ("Theta", "Θ"),
    ("Lambda", "Λ"),
    ("Xi", "Ξ"),
    ("Pi", "Π"),
    ("Sigma", "Σ"),
    ("Upsilon", "Υ"),
    ("Phi", "Φ"),
    ("Psi", "Ψ"),
    ("Omega", "Ω"),
    ("approx", "≈"),
    ("bowtie", "⋈"),
    ("times", "×"),
    ("cdot", "⋅"),
    ("pm", "±"),
    ("mp", "∓"),
    ("div", "÷"),
    ("leq", "≤"),
    ("le", "≤"),
    ("geq", "≥"),
    ("ge", "≥"),
    ("neq", "≠"),
    ("ne", "≠"),
    ("ll", "≪"),
    ("gg", "≫"),
    ("sim", "∼"),
    ("simeq", "≃"),
    ("cong", "≅"),
    ("equiv", "≡"),
    ("propto", "∝"),
    ("infty", "∞"),
    ("to", "→"),
    ("rightarrow", "→"),
    ("leftarrow", "←"),
    ("Rightarrow", "⇒"),
    ("Leftarrow", "⇐"),
    ("leftrightarrow", "↔"),
    ("Leftrightarrow", "⇔"),
    ("mapsto", "↦"),
    ("sum", "∑"),
    ("prod", "∏"),
    ("int", "∫"),
    ("partial", "∂"),
    ("nabla", "∇"),
    ("in", "∈"),
    ("notin", "∉"),
    ("ni", "∋"),
    ("subset", "⊂"),
    ("subseteq", "⊆"),
    ("supset", "⊃"),
    ("supseteq", "⊇"),
    ("cup", "∪"),
    ("cap", "∩"),
    ("setminus", "∖"),
    ("forall", "∀"),
    ("exists", "∃"),
    ("emptyset", "∅"),
    ("varnothing", "∅"),
    ("ldots", "…"),
    ("dots", "…"),
    ("cdots", "⋯"),
    ("vdots", "⋮"),
    ("mid", "|"),
    ("vert", "|"),
    ("lvert", "|"),
    ("rvert", "|"),
    ("lbrack", "["),
    ("rbrack", "]"),
    ("backslash", "\\"),
    ("gt", ">"),
    ("lt", "<"),
    ("gets", "←"),
    ("uparrow", "↑"),
    ("downarrow", "↓"),
    ("intercal", "ᵀ"),
    ("ddots", "⋱"),
    ("triangleq", "≜"),
    ("succ", "≻"),
    ("prec", "≺"),
    ("bigoplus", "⨁"),
    ("bigotimes", "⨂"),
    ("circledast", "⊛"),
    ("colonequals", "≔"),
    ("mod", " mod "),
    ("bmod", " mod "),
    ("pmod", " mod "),
    ("Vert", "‖"),
    ("langle", "⟨"),
    ("rangle", "⟩"),
    ("lfloor", "⌊"),
    ("rfloor", "⌋"),
    ("lceil", "⌈"),
    ("rceil", "⌉"),
    ("circ", "∘"),
    ("star", "⋆"),
    ("ast", "∗"),
    ("oplus", "⊕"),
    ("otimes", "⊗"),
    ("odot", "⊙"),
    ("perp", "⊥"),
    ("parallel", "∥"),
    ("angle", "∠"),
    ("degree", "°"),
    ("prime", "′"),
    ("hbar", "ℏ"),
    ("ell", "ℓ"),
    ("Re", "ℜ"),
    ("Im", "ℑ"),
    ("aleph", "ℵ"),
    ("neg", "¬"),
    ("lnot", "¬"),
    ("land", "∧"),
    ("wedge", "∧"),
    ("lor", "∨"),
    ("vee", "∨"),
    ("top", "⊤"),
    ("bot", "⊥"),
    ("models", "⊨"),
    ("vdash", "⊢"),
    ("implies", "⟹"),
    ("iff", "⟺"),
];

#[cfg(test)]
mod tests {
    use super::super::render_markdown;
    use super::*;

    #[test]
    fn arxiv_abstract_math_reads_as_text() {
        // arxiv.org/abs/2609.16338, as stored by the HTML-to-Markdown conversion (escaped
        // backslashes and underscores).
        let markdown = "Every weight is one of three symbols $\\\\{-1,0,+1\\\\}$, referenced to the information-theoretic $\\\\log\\_2 3 \\\\approx 1.585$ bits per weight; zeros account for up to $51.5\\\\%$ of all weights, the layout costs $2 - z$ bits, and the realized gain is up to $1.28\\\\times$.\n";
        let html = render_markdown(markdown);
        for formula in [
            "{\u{2212}1,0,+1}",
            "log₂ 3 ≈ 1.585",
            "51.5%",
            "2 \u{2212} z",
            "1.28×",
        ] {
            assert!(
                html.contains(&format!("<span class=\"math\">{formula}</span>")),
                "{formula}: {html}"
            );
        }
        assert!(!html.contains('$'), "{html}");
        let text = super::super::PreparedMarkdown::new(markdown)
            .plain_text()
            .to_string();
        assert!(text.contains("log₂ 3 ≈ 1.585 bits"), "{text}");
    }

    #[test]
    fn common_commands_scripts_and_display_math_translate() {
        for (tex, expected) in [
            ("x^{n+1} + y_i", "xⁿ⁺¹ + yᵢ"),
            ("\\frac{a+b}{2}", "(a+b)/2"),
            ("\\frac{\\pi}{2}", "π/2"),
            ("\\mathbb{R}^d", "ℝᵈ"),
            ("e^{i\\pi}", "e^(iπ)"),
            ("\\sqrt{x+1}", "√(x+1)"),
            ("\\hat{y} = \\mathrm{softmax}(z)", "y\u{302} = softmax(z)"),
            ("\\left( \\sum_{i=1}^n x_i \\right)", "( ∑ᵢ₌₁ⁿ xᵢ )"),
            ("O(n \\log n)", "O(n log n)"),
            ("\\text{if } x > 0", "if x > 0"),
            // rohanbansal.com writes join cardinalities with the natural-join operator.
            (
                "(cn \\bowtie mc) = 2\\text{m}, \\text{ then } \\bowtie t = 2\\text{m}",
                "(cn ⋈ mc) = 2m, then ⋈ t = 2m",
            ),
        ] {
            assert_eq!(to_unicode(tex).as_deref(), Some(expected), "{tex}");
        }
        let html = render_markdown("$$E = mc^2$$\n");
        assert!(
            html.contains("<span class=\"math math-display\">E = mc²</span>"),
            "{html}"
        );
    }

    #[test]
    fn aligned_environments_stack_rows_and_marker_letters_are_stripped() {
        assert_eq!(
            to_unicode(
                "\\begin{align*} \\text{Exp} &= \\frac{1}{N} \\\\ \\text{SD} &= 1 \\end{align*}"
            )
            .as_deref(),
            Some("Exp = 1/N\nSD = 1")
        );
        // blog.cloudflare.com wraps every formula in `m … m`.
        let html = render_markdown(
            "$$m \\begin{align*} \\text{Exp} &= \\frac{1}{N} \\\\\\\\ \\text{SD} &= 1 \\end{align*}  m$$\n\nSo $m2^k = 8m$ rings.\n",
        );
        assert!(
            html.contains("<span class=\"math math-display\">Exp = 1/N<br>SD = 1</span>"),
            "{html}"
        );
        assert!(
            html.contains("<span class=\"math\">2ᵏ = 8</span>"),
            "{html}"
        );
        // Without that evidence a letter at both ends is part of the formula.
        let html = render_markdown("The mass $m = 2m$ doubles.\n");
        assert!(html.contains(">m = 2m<"), "{html}");
    }

    #[test]
    fn unknown_tex_stays_as_source_in_a_code_span() {
        let html = render_markdown("$\\begin{tikzpicture} \\draw (0,0); \\end{tikzpicture}$\n");
        assert!(html.contains("<code class=\"math\">"), "{html}");
        // A matrix reads as its values, inside the delimiters that say it is one.
        let matrix = render_markdown("$\\begin{bmatrix} 1 & 0 \\end{bmatrix}$\n");
        assert!(
            matrix.contains("<span class=\"math\">[1 0]</span>"),
            "{matrix}"
        );
        let vector = render_markdown("$\\begin{pmatrix} x \\end{pmatrix}$\n");
        assert!(
            vector.contains("<span class=\"math\">(x)</span>"),
            "{vector}"
        );
        assert_eq!(to_unicode("\\sqrt[3]{x}"), None);
        assert_eq!(to_unicode("{unclosed"), None);
        assert_eq!(to_unicode("unopened}"), None);
    }

    #[test]
    fn prose_between_dollar_signs_is_left_alone() {
        for markdown in [
            "That is $AAPL/$MSFT for you.\n",
            "It costs $5 and $10 each.\n",
            "Run `$x$` in a shell.\n",
            "A price of $1,000 and then $2,000 later.\n",
        ] {
            let html = render_markdown(markdown);
            assert!(!html.contains("class=\"math"), "{html}");
            assert!(html.contains('$'), "{html}");
        }
        assert!(render_markdown("That is $AAPL/$MSFT for you.\n").contains("$AAPL/$MSFT"));
        assert!(looks_like_tex("2 - z"));
        assert!(looks_like_tex("n"));
        assert!(!looks_like_tex("AAPL/"));
        assert!(!looks_like_tex(" "));
    }
}
