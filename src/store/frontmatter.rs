//! `---` YAML front matter, the framing GitHub renders as a table on blob pages.

use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};

const FENCE: &str = "---";

/// Split a document into its YAML block and body. The body keeps its trailing newline.
pub fn split(text: &str) -> Result<(&str, &str)> {
    let rest = text
        .strip_prefix(FENCE)
        .and_then(|rest| {
            rest.strip_prefix('\n')
                .or_else(|| rest.strip_prefix("\r\n"))
        })
        .context("missing `---` front matter")?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == FENCE {
            let yaml = &rest[..offset];
            let body = &rest[offset + line.len()..];
            return Ok((yaml, body.strip_prefix('\n').unwrap_or(body)));
        }
        offset += line.len();
    }
    bail!("unterminated `---` front matter")
}

pub fn join(yaml: &str, body: &str) -> String {
    let yaml = yaml.trim_end_matches('\n');
    let body = body.trim_start_matches('\n');
    if body.is_empty() {
        format!("{FENCE}\n{yaml}\n{FENCE}\n")
    } else {
        format!("{FENCE}\n{yaml}\n{FENCE}\n\n{body}")
    }
}

pub fn parse<T: DeserializeOwned>(text: &str) -> Result<(T, &str)> {
    let (yaml, body) = split(text)?;
    let front = serde_yaml_ng::from_str(yaml).context("invalid front matter")?;
    Ok((front, body))
}

pub fn render<T: Serialize>(front: &T, body: &str) -> Result<String> {
    let yaml = serde_yaml_ng::to_string(front).context("serializing front matter")?;
    Ok(join(&yaml, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Front {
        title: String,
        #[serde(default)]
        tags: Vec<String>,
    }

    #[test]
    fn splits_and_joins() {
        let doc = "---\ntitle: Hi\n---\n\nBody line 1\n\n---\nnot a fence\n";
        let (yaml, body) = split(doc).unwrap();
        assert_eq!(yaml, "title: Hi\n");
        assert_eq!(body, "Body line 1\n\n---\nnot a fence\n");
        assert_eq!(join(yaml, body), doc);
    }

    #[test]
    fn handles_crlf_and_empty_body() {
        let (yaml, body) = split("---\r\ntitle: Hi\r\n---\r\n").unwrap();
        assert_eq!(yaml, "title: Hi\r\n");
        assert_eq!(body, "");
        assert_eq!(join("a: 1", ""), "---\na: 1\n---\n");
    }

    #[test]
    fn rejects_missing_or_unterminated() {
        assert!(split("title: Hi\n").is_err());
        assert!(split("---\ntitle: Hi\n").is_err());
        assert!(split("--- \ntitle: Hi\n---\n").is_err());
    }

    #[test]
    fn round_trips_typed() {
        let front = Front {
            title: "Quotes: \"and\" colons".into(),
            tags: vec!["a".into(), "b".into()],
        };
        let text = render(&front, "# Heading\n").unwrap();
        assert!(text.starts_with("---\ntitle: "), "{text}");
        let (back, body): (Front, _) = parse(&text).unwrap();
        assert_eq!(back, front);
        assert_eq!(body, "# Heading\n");
    }

    #[test]
    fn reports_invalid_yaml() {
        let err = parse::<Front>("---\ntitle: [\n---\n").unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid front matter"),
            "{err:#}"
        );
    }
}
