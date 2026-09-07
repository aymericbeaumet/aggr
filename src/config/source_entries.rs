use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::SourceConfig;

#[derive(Deserialize)]
#[serde(untagged)]
enum Entries {
    Text(String),
    List(Vec<String>),
}

pub(super) fn split_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|line| !line.is_empty())
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
    if let Some(include) = table.remove("include") {
        if table.contains_key("url") {
            bail!("`url` and its legacy alias `include` are mutually exclusive");
        }
        table.insert("url".into(), include);
    }
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
