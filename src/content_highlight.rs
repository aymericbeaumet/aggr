//! Bounded build-time highlighting. Explicit publisher hints always outrank inference.

use std::sync::OnceLock;

use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::{SyntaxReference, SyntaxSet};

pub(super) fn write(
    output: &mut dyn std::fmt::Write,
    language: Option<&str>,
    code: &str,
) -> std::fmt::Result {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    let syntaxes = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
    let bounded = code.len() <= 100 * 1024 && code.lines().all(|line| line.len() <= 2_000);
    let hint = language.unwrap_or_default().trim();
    let syntax = if hint.is_empty() {
        bounded.then(|| infer(syntaxes, code)).flatten()
    } else {
        explicit(syntaxes, hint)
    };
    let label = syntax.map_or("Text", |syntax| match syntax.name.as_str() {
        "Bourne Again Shell (bash)" => "Shell",
        "Plain Text" => "Text",
        name => name,
    });
    write!(
        output,
        "<span class=\"code-snippet\" data-language=\"{}\">",
        super::escape_html(label)
    )?;
    let highlighted = syntax.filter(|_| bounded).and_then(|syntax| {
        let mut generator = ClassedHTMLGenerator::new_with_class_style(
            syntax,
            syntaxes,
            ClassStyle::SpacedPrefixed { prefix: "syntax-" },
        );
        for line in syntect::util::LinesWithEndings::from(code) {
            generator
                .parse_html_for_line_which_includes_newline(line)
                .ok()?;
        }
        Some(generator.finalize())
    });
    output.write_str(&highlighted.unwrap_or_else(|| super::escape_html(code)))?;
    output.write_str("</span>")
}

fn explicit<'a>(syntaxes: &'a SyntaxSet, hint: &str) -> Option<&'a SyntaxReference> {
    let token = hint.split_whitespace().next()?.to_ascii_lowercase();
    let token = token.strip_prefix("language-").unwrap_or(&token);
    let token = token
        .strip_prefix("{.")
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(token);
    let token = match token {
        "text" | "txt" | "plain" | "plaintext" | "none" => return None,
        "shell" | "shellscript" | "sh" | "zsh" | "console" => "bash",
        "node" | "nodejs" | "javascript" => "js",
        "python3" => "py",
        "c++" => "cpp",
        "csharp" => "cs",
        _ => token,
    };
    syntaxes
        .find_syntax_by_token(token)
        .filter(|syntax| syntax.name != "Plain Text")
}

fn infer<'a>(syntaxes: &'a SyntaxSet, code: &str) -> Option<&'a SyntaxReference> {
    let text = code.trim();
    let first = text.lines().next()?;
    // Shebangs and XML declarations carry an actual language hint. Arbitrary prose does not.
    if (first.starts_with("#!") || first.starts_with("<?xml"))
        && let Some(syntax) = syntaxes.find_syntax_by_first_line(first)
    {
        return Some(syntax);
    }
    if (text.starts_with('{') || text.starts_with('['))
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(text)
        && match value {
            serde_json::Value::Object(ref object) => !object.is_empty(),
            serde_json::Value::Array(ref array) => !array.is_empty(),
            _ => false,
        }
    {
        return explicit(syntaxes, "json");
    }
    let lines = text.lines().map(str::trim).collect::<Vec<_>>();
    let token = if lines.iter().any(|line| {
        let line = line.strip_prefix("pub ").unwrap_or(line);
        let line = line.strip_prefix("async ").unwrap_or(line);
        line.starts_with("fn ") && line.contains('(') && line.contains('{')
    }) {
        "rust"
    } else if lines.iter().any(|line| {
        let line = line.strip_prefix("async ").unwrap_or(line);
        line.starts_with("def ") && line.contains('(') && line.ends_with(':')
    }) {
        "python"
    } else if lines.iter().any(|line| line.starts_with("package "))
        && lines
            .iter()
            .any(|line| line.starts_with("func ") && line.contains('('))
    {
        "go"
    } else if lines
        .iter()
        .any(|line| line.starts_with("function ") && line.contains('(') && line.contains('{'))
    {
        "js"
    } else if text.to_ascii_lowercase().starts_with("<!doctype html") {
        "html"
    } else {
        let upper = text.to_ascii_uppercase();
        if (upper.starts_with("SELECT ")
            && upper.contains(" FROM ")
            && (upper.contains(';') || upper.contains('*') || upper.contains(" WHERE ")))
            || upper.starts_with("CREATE TABLE ")
            || (upper.starts_with("INSERT INTO ") && upper.contains("VALUES"))
        {
            "sql"
        } else {
            return None;
        }
    };
    explicit(syntaxes, token)
}
