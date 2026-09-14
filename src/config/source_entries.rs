use anyhow::{Context, Result};
use serde::Deserialize;

use super::SourceConfig;

#[derive(Deserialize)]
#[serde(untagged)]
enum Entries {
    Text(String),
    List(Vec<String>),
}

pub(super) fn split_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(|line| line.split_once(" #").map_or(line, |(value, _)| value))
        .map(str::trim)
        .filter(|line| !line.is_empty())
}

pub(super) fn deserialize_sources<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<SourceConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let tables = Vec::<toml::Table>::deserialize(deserializer)?;
    tables
        .into_iter()
        .enumerate()
        .try_fold(Vec::new(), |mut sources, (index, table)| {
            sources.extend(
                expand_entry(table).with_context(|| format!("[[sources]] #{}", index + 1))?,
            );
            Ok::<_, anyhow::Error>(sources)
        })
        .map_err(|error| serde::de::Error::custom(format!("{error:#}")))
}

fn expand_entry(mut table: toml::Table) -> Result<Vec<SourceConfig>> {
    let Some(value) = table.remove("url") else {
        return Ok(vec![table.try_into()?]);
    };
    // Validate options even when the group is empty.
    let defaults: SourceConfig = table.try_into()?;
    let entries = match value
        .try_into::<Entries>()
        .context("`url` must be a string or an array of strings")?
    {
        Entries::Text(text) => vec![text],
        Entries::List(entries) => entries,
    };
    if !entries
        .iter()
        .any(|entry| split_lines(entry).next().is_some())
    {
        super::validate_source_options(&defaults)?;
    }
    Ok(entries
        .iter()
        .flat_map(|entry| split_lines(entry))
        .map(|value| {
            let mut source = defaults.clone();
            source.url = Some(value.to_owned());
            source
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_are_removed_from_raw_lines_before_whitespace_is_trimmed() {
        assert_eq!(
            split_lines(" #comment\n  https://foobar # test\n\t # another\n#literal\nhttps://example.org/#fragment\n").collect::<Vec<_>>(),
            ["https://foobar", "#literal", "https://example.org/#fragment"]
        );
    }

    #[test]
    fn string_array_and_multiline_forms_share_comment_ordering() {
        for value in [
            r#"" #comment\n  https://foobar # test\n # end""#,
            r#"[" #comment", "  https://foobar # test", " # end"]"#,
            "''' #comment\n  https://foobar # test\n # end'''",
        ] {
            {
                let key = "url";
                let table = toml::from_str(&format!("{key} = {value}\ncategory = 'News'")).unwrap();
                let sources = expand_entry(table).unwrap();
                assert_eq!(sources.len(), 1, "{key} = {value}");
                assert_eq!(sources[0].url.as_deref(), Some("https://foobar"));
                assert_eq!(sources[0].category.as_deref(), Some("News"));
            }
        }
    }

    #[test]
    fn comments_require_an_ascii_space_and_preserve_fragments() {
        assert_eq!(
            split_lines(" https://example.org/feed#part # a comment\r\n # full comment\nhttps://example.org/next\t#fragment\n").collect::<Vec<_>>(),
            ["https://example.org/feed#part", "https://example.org/next\t#fragment"]
        );
    }
}
