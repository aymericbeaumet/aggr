//! Discussion search targets: built-in providers and custom URL templates.

use std::collections::BTreeSet;

use serde::de::Deserializer;
use serde::{Deserialize, Serialize};

use super::NetworkProvider;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetworkConfig {
    pub name: String,
    pub url: String,
    /// Optional build-time lookup. Item contexts expose only exact matches.
    pub provider: Option<NetworkProvider>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNetworkConfig {
    #[serde(default)]
    provider: Option<NetworkProvider>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

impl<'de> Deserialize<'de> for NetworkConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Value {
            Name(String),
            Table(RawNetworkConfig),
        }
        match Value::deserialize(deserializer)? {
            Value::Name(name) => NetworkProvider::from_name(&name)
                .map(NetworkProvider::network)
                .ok_or_else(|| unknown_discussion(&name))
                .map_err(serde::de::Error::custom),
            Value::Table(raw) => raw.into_config().map_err(serde::de::Error::custom),
        }
    }
}

impl RawNetworkConfig {
    fn into_config(self) -> Result<NetworkConfig, String> {
        if let Some(provider) = self.provider {
            if self.name.is_some() || self.url.is_some() {
                return Err(
                    "a built-in discussion accepts only `provider`; use a name string or `name` + `url` for a custom one"
                        .into(),
                );
            }
            return Ok(provider.network());
        }
        match (self.name, self.url) {
            (Some(name), Some(url)) if !name.trim().is_empty() && !url.trim().is_empty() => {
                Ok(NetworkConfig {
                    name,
                    url,
                    provider: None,
                })
            }
            _ => Err(
                "set `provider` for a built-in discussion, or both `name` and `url` for a custom one"
                    .into(),
            ),
        }
    }
}

pub(super) fn deserialize_discussions<'de, D>(
    deserializer: D,
) -> Result<Vec<NetworkConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    let discussions = Vec::<NetworkConfig>::deserialize(deserializer)?;
    unique_discussions(&discussions).map_err(serde::de::Error::custom)?;
    Ok(discussions)
}

pub(super) fn deserialize_optional_discussions<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<NetworkConfig>>, D::Error>
where
    D: Deserializer<'de>,
{
    let discussions = Option::<Vec<NetworkConfig>>::deserialize(deserializer)?;
    if let Some(discussions) = &discussions {
        unique_discussions(discussions).map_err(serde::de::Error::custom)?;
    }
    Ok(discussions)
}

/// `[[networks]]` and `networks = ...` are the previous spelling.
pub(super) fn reject_legacy_networks<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: Deserializer<'de>,
{
    serde::de::IgnoredAny::deserialize(deserializer)?;
    Err(serde::de::Error::custom(
        "`networks` has moved to `discussions`; set discussions = [\"hackernews\", \"reddit\"] in [defaults] or on a [[sources]] table",
    ))
}

fn unique_discussions(discussions: &[NetworkConfig]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for discussion in discussions {
        let mut keys = vec![discussion.name.to_ascii_lowercase()];
        if let Some(provider) = discussion.provider {
            keys.push(provider.as_str().to_string());
        }
        if keys.iter().any(|key| seen.contains(key)) {
            return Err(format!("discussion {:?} is listed twice", discussion.name));
        }
        seen.extend(keys);
    }
    Ok(())
}

fn unknown_discussion(name: &str) -> String {
    format!(
        "unknown discussion {name:?}; expected \"hackernews\", \"reddit\", \"x\", or a {{ name, url }} table"
    )
}
