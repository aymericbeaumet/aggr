//! `---` YAML front matter, the framing GitHub renders as a table on blob pages.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use serde_yaml_ng::Value;

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

/// Front matter with a free-form `extra` block. Typed fields only hold plain JSON-compatible
/// data; `extra` is the one place YAML that serde_json rejects can enter an item, so `parse`
/// sanitizes it and every JSON artifact derived from an item stays infallible.
pub trait Extra {
    fn extra_mut(&mut self) -> Option<&mut BTreeMap<String, Value>> {
        None
    }
}

impl Extra for crate::model::FrontMatter {
    fn extra_mut(&mut self) -> Option<&mut BTreeMap<String, Value>> {
        Some(&mut self.extra)
    }
}

pub fn parse<T: DeserializeOwned + Extra>(text: &str) -> Result<(T, &str)> {
    let (yaml, body) = split(text)?;
    let mut front: T = serde_yaml_ng::from_str(yaml).context("invalid front matter")?;
    if let Some(extra) = front.extra_mut() {
        *extra = json_safe_extra(std::mem::take(extra));
    }
    Ok((front, body))
}

/// `extra` with everything JSON cannot carry removed; see [`json_safe`].
pub fn json_safe_extra(extra: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    extra
        .into_iter()
        .map(|(key, value)| (key, json_safe(value)))
        .collect()
}

/// The closest value serde_json can serialize: mapping entries keyed by null, a sequence or a
/// mapping are dropped, scalar keys become the strings JSON prints for them, and non-finite
/// floats become null (what serde_json emits for them anyway). Everything else is unchanged.
pub fn json_safe(value: Value) -> Value {
    match value {
        Value::Number(number) if !number.is_finite() => Value::Null,
        Value::Sequence(items) => Value::Sequence(items.into_iter().map(json_safe).collect()),
        Value::Mapping(mapping) => Value::Mapping(
            mapping
                .into_iter()
                .filter_map(|(key, value)| Some((Value::String(json_key(key)?), json_safe(value))))
                .collect(),
        ),
        Value::Tagged(tagged) => Value::Tagged(Box::new(serde_yaml_ng::value::TaggedValue {
            tag: tagged.tag,
            value: json_safe(tagged.value),
        })),
        other => other,
    }
}

/// The string serde_json writes for a map key, or `None` when it would refuse the key.
fn json_key(key: Value) -> Option<String> {
    match key {
        Value::String(key) => Some(key),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) if number.is_finite() => serde_json::to_string(&number).ok(),
        _ => None,
    }
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

    impl Extra for Front {}

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

    #[test]
    fn json_safe_keeps_what_json_carries_and_drops_the_rest() {
        let value: Value = serde_yaml_ng::from_str(concat!(
            "plain: text\n",
            "1: int key\n",
            "true: bool key\n",
            "1.5: float key\n",
            "null: dropped\n",
            "[a]: dropped\n",
            "{a: b}: dropped\n",
            ".nan: dropped\n",
            "ratio: .nan\n",
            "limit: -.inf\n",
            "list: [1, .inf, {null: x, ok: y}]\n",
            "tagged: !custom {null: x, kept: 1}\n",
        ))
        .unwrap();
        assert!(serde_json::to_string(&value).is_err());

        let safe = serde_json::to_value(json_safe(value)).unwrap();
        assert_eq!(
            safe,
            serde_json::json!({
                "plain": "text",
                "1": "int key",
                "true": "bool key",
                "1.5": "float key",
                "ratio": null,
                "limit": null,
                "list": [1, null, {"ok": "y"}],
                "tagged": {"!custom": {"kept": 1}},
            })
        );
    }

    #[test]
    fn parse_sanitizes_free_form_extra_without_touching_typed_fields() {
        let text = concat!(
            "---\n",
            "title: 2024\n",
            "link: https://example.com/\n",
            "source: s\n",
            "first_seen: 2026-09-16T00:00:00Z\n",
            "extra:\n",
            "  junk:\n",
            "    null: x\n",
            "    kept: y\n",
            "  ratio: .nan\n",
            "---\n",
            "body\n",
        );
        let (front, body) = parse::<crate::model::FrontMatter>(text).unwrap();
        // Typed fields keep YAML's plain-scalar rules: an unquoted number is still a title.
        assert_eq!(front.title, "2024");
        assert_eq!(body, "body\n");
        assert_eq!(
            serde_json::to_value(&front.extra).unwrap(),
            serde_json::json!({"junk": {"kept": "y"}, "ratio": null})
        );
    }
}
