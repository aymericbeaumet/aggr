//! Bounded build-time highlighting. Explicit publisher hints always outrank inference, and a
//! snippet whose language stays unknown is labelled with nothing rather than guessed at.

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
    let detected = match language.and_then(hint_token) {
        // The publisher's own word for the language wins: a grammar may only be a close relative,
        // and a language Sublime has no grammar for still deserves its name on the block.
        Some(token) => Some(Detected {
            syntax: grammar(syntaxes, &token),
            label: display_name(&token),
        }),
        None => bounded.then(|| infer(syntaxes, code)).flatten(),
    };
    let syntax = detected.as_ref().and_then(|detected| detected.syntax);
    let label = detected
        .map(|detected| detected.label)
        .filter(|label| !label.is_empty());
    match label {
        Some(label) => write!(
            output,
            "<span class=\"code-snippet\" data-language=\"{}\">",
            super::escape_html(&label)
        )?,
        None => output.write_str("<span class=\"code-snippet\">")?,
    }
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

/// The normalized language token a publisher declared, or `None` when there is no hint or the hint
/// explicitly says the block is plain text.
fn hint_token(hint: &str) -> Option<String> {
    let token = hint.split_whitespace().next()?.to_ascii_lowercase();
    let token = token.strip_prefix("language-").unwrap_or(&token);
    let token = token.strip_prefix("lang-").unwrap_or(token);
    let token = token
        .strip_prefix("{.")
        .and_then(|token| token.strip_suffix('}'))
        .unwrap_or(token);
    let token =
        token.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '+' && ch != '#');
    // The hint is printed back to the reader, so it has to look like a language and nothing else.
    (!token.is_empty()
        && token.len() <= 24
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+#._-".contains(&byte))
        && !matches!(token, "text" | "txt" | "plain" | "plaintext" | "none"))
    .then(|| token.to_string())
}

/// Sublime's default grammars do not cover every language a publisher names. Where a close
/// relative colours the same syntax correctly it is used; otherwise the block stays plain.
fn grammar<'a>(syntaxes: &'a SyntaxSet, token: &str) -> Option<&'a SyntaxReference> {
    let token = match token {
        "shell" | "shellscript" | "sh" | "zsh" | "console" | "terminal" => "bash",
        "node" | "nodejs" | "javascript" | "mjs" | "cjs" => "js",
        "typescript" | "ts" | "tsx" | "jsx" => "js",
        "python3" | "python2" => "py",
        "c++" => "cpp",
        "csharp" => "cs",
        "scss" | "sass" | "less" => "css",
        "golang" => "go",
        "yml" => "yaml",
        "htm" | "vue" | "svelte" => "html",
        "patch" => "diff",
        "kotlin" | "kt" | "groovy" => "java",
        other => other,
    };
    syntaxes
        .find_syntax_by_token(token)
        .filter(|syntax| syntax.name != "Plain Text")
}

/// How the language is written for a reader.
fn display_name(token: &str) -> String {
    for (candidate, label) in LANGUAGE_LABELS {
        if token.eq_ignore_ascii_case(candidate) {
            return (*label).to_string();
        }
    }
    let mut chars = token.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

const LANGUAGE_LABELS: &[(&str, &str)] = &[
    ("plain text", ""),
    ("bourne again shell (bash)", "Shell"),
    ("bash", "Shell"),
    ("sh", "Shell"),
    ("shell", "Shell"),
    ("shellscript", "Shell"),
    ("zsh", "Shell"),
    ("console", "Shell"),
    ("terminal", "Shell"),
    ("js", "JavaScript"),
    ("mjs", "JavaScript"),
    ("cjs", "JavaScript"),
    ("node", "JavaScript"),
    ("nodejs", "JavaScript"),
    ("jsx", "JSX"),
    ("ts", "TypeScript"),
    ("tsx", "TSX"),
    ("py", "Python"),
    ("python3", "Python"),
    ("python2", "Python"),
    ("rb", "Ruby"),
    ("golang", "Go"),
    ("cpp", "C++"),
    ("c++", "C++"),
    ("cs", "C#"),
    ("csharp", "C#"),
    ("objc", "Objective-C"),
    ("kt", "Kotlin"),
    ("rs", "Rust"),
    ("yml", "YAML"),
    ("yaml", "YAML"),
    ("toml", "TOML"),
    ("json", "JSON"),
    ("jsonc", "JSON"),
    ("json5", "JSON5"),
    ("sql", "SQL"),
    ("html", "HTML"),
    ("htm", "HTML"),
    ("xml", "XML"),
    ("css", "CSS"),
    ("scss", "SCSS"),
    ("sass", "Sass"),
    ("less", "Less"),
    ("md", "Markdown"),
    ("markdown", "Markdown"),
    ("ini", "INI"),
    ("csv", "CSV"),
    ("tsv", "TSV"),
    ("http", "HTTP"),
    ("graphql", "GraphQL"),
    ("gql", "GraphQL"),
    ("proto", "Protocol Buffers"),
    ("hcl", "HCL"),
    ("tf", "Terraform"),
    ("ps1", "PowerShell"),
    ("powershell", "PowerShell"),
    ("dockerfile", "Dockerfile"),
    ("docker", "Dockerfile"),
    ("make", "Makefile"),
    ("makefile", "Makefile"),
    ("diff", "Diff"),
    ("patch", "Diff"),
    ("php", "PHP"),
    ("vb", "Visual Basic"),
    ("asm", "Assembly"),
    ("tex", "LaTeX"),
    ("latex", "LaTeX"),
    ("matlab", "MATLAB"),
    ("ocaml", "OCaml"),
    ("fsharp", "F#"),
    ("nix", "Nix"),
    ("zig", "Zig"),
];

struct Detected<'a> {
    syntax: Option<&'a SyntaxReference>,
    label: String,
}

impl<'a> Detected<'a> {
    fn named(syntaxes: &'a SyntaxSet, token: &str) -> Self {
        Self {
            syntax: grammar(syntaxes, token),
            label: display_name(token),
        }
    }
}

fn infer<'a>(syntaxes: &'a SyntaxSet, code: &str) -> Option<Detected<'a>> {
    let text = code.trim();
    let first = text.lines().next()?;
    // Shebangs and XML declarations carry an actual language hint. Arbitrary prose does not.
    if (first.starts_with("#!") || first.starts_with("<?xml"))
        && let Some(syntax) = syntaxes.find_syntax_by_first_line(first)
    {
        return Some(Detected {
            syntax: Some(syntax),
            label: display_name(&syntax.name),
        });
    }
    if (text.starts_with('{') || text.starts_with('['))
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(text)
        && match value {
            serde_json::Value::Object(ref object) => !object.is_empty(),
            serde_json::Value::Array(ref array) => !array.is_empty(),
            _ => false,
        }
    {
        return Some(Detected::named(syntaxes, "json"));
    }
    Some(Detected::named(syntaxes, classify(text)?))
}

/// A key that ends in `:` with a more indented line under it: a YAML mapping, not a header list.
fn nests_under_key(text: &str) -> bool {
    let indent = |line: &str| line.len() - line.trim_start().len();
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines.windows(2).any(|pair| {
        let key = pair[0].trim_end();
        key.ends_with(':')
            && !key.trim_start().starts_with('#')
            && indent(pair[1]) > indent(pair[0])
    })
}

/// Score every language on markers that prose and neighbouring languages do not share, and accept
/// the winner only when it is unambiguous: colouring a snippet as the wrong language is worse than
/// leaving it plain.
fn classify(text: &str) -> Option<&'static str> {
    let lines: Vec<&str> = text.lines().map(str::trim_start).collect();
    let starts = |prefix: &str| {
        lines
            .iter()
            .filter(|line| line.starts_with(prefix))
            .count()
            .min(3) as i32
    };
    let ends = |suffix: &str| {
        lines
            .iter()
            .filter(|line| line.trim_end().ends_with(suffix))
            .count()
            .min(3) as i32
    };
    let has = |needle: &str| i32::from(text.contains(needle));
    let upper = text.to_ascii_uppercase();

    let scores = [
        (
            "diff",
            3 * starts("@@ ")
                + 2 * starts("--- a/")
                + 2 * starts("+++ b/")
                + 3 * starts("diff --git "),
        ),
        (
            "dockerfile",
            3 * starts("FROM ")
                + 2 * starts("RUN ")
                + starts("ARG ")
                + starts("ENV ")
                + starts("COPY ")
                + starts("ADD ")
                + starts("USER ")
                + starts("WORKDIR ")
                + starts("ENTRYPOINT ")
                + starts("CMD ")
                + starts("EXPOSE "),
        ),
        (
            "rust",
            2 * starts("fn ")
                + 2 * starts("pub fn ")
                + 2 * starts("async fn ")
                + 2 * starts("impl ")
                + 2 * starts("pub struct ")
                + has("-> Result<")
                + has("&str")
                + has("println!")
                + has("let mut ")
                + has("use std::")
                + has("#[derive("),
        ),
        (
            "py",
            2 * starts("def ")
                + 2 * starts("async def ")
                + 2 * starts("from ") * has(" import ")
                + has("__name__")
                + has("self.")
                + has("elif ")
                + 2 * ends("):"),
        ),
        (
            "go",
            3 * starts("package ") * starts("func ")
                + 2 * starts("func ")
                + has(":= ")
                + has("fmt.")
                + has("err != nil")
                + has("go func("),
        ),
        (
            "js",
            3 * starts("function ")
                + 2 * starts("const ")
                + 2 * starts("export ")
                + starts("import ") * has(" from '")
                + has("=> {")
                + has("console.log(")
                + has("document.")
                + has("async function")
                + has("require("),
        ),
        (
            "java",
            2 * starts("public class ")
                + 2 * starts("private ")
                + starts("import java")
                + has("System.out.print")
                + has("public static void main"),
        ),
        (
            "c",
            3 * starts("#include <") + starts("#define ") + has("int main(") + has("printf("),
        ),
        (
            "cpp",
            2 * has("#include <iostream>")
                + has("std::")
                + has("namespace ")
                + has("template<")
                + has("template <"),
        ),
        (
            "php",
            3 * starts("<?php") + has("$this->") + has("function ") * has("$"),
        ),
        (
            "rb",
            2 * starts("def ") * ends("end")
                + starts("require '")
                + has("puts ")
                + has("do |")
                + has(".each do"),
        ),
        (
            "html",
            2 * i32::from(text.to_ascii_lowercase().starts_with("<!doctype html"))
                + has("</div>")
                + has("</html>")
                + has("</body>")
                + has("<span ")
                + has("<a href="),
        ),
        (
            "css",
            2 * ends(" {") * has(";")
                + has("color:")
                + has("margin:")
                + has("padding:")
                + has("display:")
                + has("@media "),
        ),
        (
            "yaml",
            2 * starts("- ") * has(": ")
                + 2 * starts("---")
                + has("\n  - ")
                // Bare `Key: value` lines are just as likely to be a mail header or a log, so the
                // deciding signal is a mapping that nests underneath its key.
                + 2 * i32::from(
                    lines
                        .iter()
                        .filter(|line| {
                            line.split_once(": ").is_some_and(|(key, _)| {
                                !key.is_empty()
                                    && key
                                        .chars()
                                        .all(|ch| ch.is_alphanumeric() || "-_.".contains(ch))
                            })
                        })
                        .count()
                        >= 3,
                )
                + 2 * i32::from(nests_under_key(text)),
        ),
        (
            "toml",
            3 * i32::from(
                lines
                    .iter()
                    .any(|line| line.starts_with('[') && line.trim_end().ends_with(']')),
            ) * i32::from(lines.iter().filter(|line| line.contains(" = ")).count() >= 2),
        ),
        (
            "make",
            3 * i32::from(
                text.lines()
                    .any(|line| line.starts_with('\t') && !line.trim().is_empty()),
            ) * i32::from(lines.iter().any(|line| {
                line.split_once(':').is_some_and(|(target, _)| {
                    !target.is_empty()
                        && target
                            .chars()
                            .all(|ch| ch.is_alphanumeric() || "-_. $()%".contains(ch))
                })
            })),
        ),
        (
            "bash",
            2 * starts("$ ")
                + starts("sudo ")
                + starts("git ")
                + starts("npm ")
                + starts("npx ")
                + starts("yarn ")
                + starts("pnpm ")
                + starts("cargo ")
                + starts("docker ")
                + starts("kubectl ")
                + starts("curl ")
                + starts("apt ")
                + starts("apt-get ")
                + starts("brew ")
                + starts("pip ")
                + starts("cd ")
                + starts("mkdir ")
                + starts("export ")
                + starts("echo ")
                + has("&& \\")
                + has("| grep "),
        ),
        (
            "sql",
            2 * i32::from(upper.starts_with("SELECT ") && upper.contains(" FROM "))
                + 2 * i32::from(upper.starts_with("CREATE TABLE "))
                + 2 * i32::from(upper.starts_with("INSERT INTO ") && upper.contains("VALUES"))
                + i32::from(upper.contains(" WHERE "))
                + i32::from(upper.contains(" JOIN "))
                + i32::from(upper.contains(" GROUP BY ")),
        ),
        (
            "lua",
            2 * starts("local ") + has("function ") * ends("end") + has("nil"),
        ),
        (
            "haskell",
            2 * starts("module ") * has("where") + has(":: ") * has("->") + starts("import Data."),
        ),
    ];

    let mut ranked: Vec<_> = scores
        .into_iter()
        .filter(|(_, score)| *score >= 3)
        .collect();
    ranked.sort_by_key(|(_, score)| -score);
    match ranked.as_slice() {
        [(language, best), rest @ ..] if rest.first().is_none_or(|(_, next)| best > next) => {
            Some(language)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(language: Option<&str>, code: &str) -> String {
        let mut out = String::new();
        write(&mut out, language, code).unwrap();
        out
    }

    fn label(html: &str) -> Option<String> {
        let start = html.find("data-language=\"")? + "data-language=\"".len();
        Some(html[start..start + html[start..].find('"')?].to_string())
    }

    #[test]
    fn an_unrecognized_block_carries_no_language_label() {
        for code in [
            "just some prose that happens to sit in a code block",
            "1 2 3\n4 5 6\n",
            "",
        ] {
            let html = snippet(None, code);
            assert!(label(&html).is_none(), "{code:?} -> {html}");
        }
        assert!(label(&snippet(Some("text"), "anything")).is_none());
        assert!(label(&snippet(Some("plaintext"), "anything")).is_none());
    }

    #[test]
    fn a_declared_language_keeps_its_own_name_even_without_a_grammar() {
        for (hint, expected) in [
            ("dockerfile", "Dockerfile"),
            ("language-ts", "TypeScript"),
            ("zsh", "Shell"),
            ("zig", "Zig"),
            ("{.toml}", "TOML"),
        ] {
            assert_eq!(
                label(&snippet(Some(hint), "FROM scratch")).as_deref(),
                Some(expected),
                "{hint}"
            );
        }
    }

    #[test]
    fn typescript_is_coloured_with_the_javascript_grammar() {
        let html = snippet(Some("ts"), "const answer: number = 42;\n");
        assert!(html.contains("syntax-"), "{html}");
        assert_eq!(label(&html).as_deref(), Some("TypeScript"));
    }

    #[test]
    fn inference_recognizes_distinctive_snippets_without_a_hint() {
        let cases = [
            (
                "#include <stdio.h>\nint main(void) { printf(\"hi\"); }\n",
                "C",
            ),
            (
                "FROM rust:1\nRUN cargo build\nCOPY . /app\nWORKDIR /app\n",
                "Dockerfile",
            ),
            (
                "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n",
                "Diff",
            ),
            (
                "$ git clone https://example.com/repo\n$ cd repo\n$ cargo build --release\n",
                "Shell",
            ),
            (
                "pub fn main() {\n    let mut total = 0;\n    println!(\"{total}\");\n}\n",
                "Rust",
            ),
            (
                "name: build\non:\n  push:\n    branches: [main]\njobs:\n  test:\n    runs-on: ubuntu\n",
                "YAML",
            ),
            (
                "[package]\nname = \"aggr\"\nversion = \"1.0.0\"\nedition = \"2024\"\n",
                "TOML",
            ),
        ];
        for (code, expected) in cases {
            assert_eq!(
                label(&snippet(None, code)).as_deref(),
                Some(expected),
                "{code}"
            );
        }
    }

    #[test]
    fn inference_declines_prose_and_ambiguous_fragments() {
        for code in [
            "The quick brown fox jumps over the lazy dog, repeatedly and at length.\n",
            "foo\nbar\nbaz\n",
            "Error: could not find the file you asked for\n",
        ] {
            assert!(label(&snippet(None, code)).is_none(), "{code}");
        }
    }

    #[test]
    fn oversized_snippets_are_neither_highlighted_nor_labelled_by_inference() {
        let huge = "x".repeat(200 * 1024);
        let html = snippet(None, &huge);
        assert!(label(&html).is_none());
        assert!(!html.contains("syntax-"));
    }
}
