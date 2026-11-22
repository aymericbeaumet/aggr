//! Policy evictions can be reconsidered after configuration changes without undoing manual deletion.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};

use super::Store;
use crate::config::{
    ContentMode, Defaults, DocumentPolicy, ImagePolicy, Limits, PreviewPolicy, Source,
};
use crate::model::{Item, RawItem, dedupe_keys, sha1_hex};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Evictions {
    items: BTreeMap<String, Eviction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Eviction {
    keys: Vec<String>,
    policy: String,
    first_seen: chrono::DateTime<chrono::Utc>,
}

fn policy(
    content: ContentMode,
    images: ImagePolicy,
    previews: PreviewPolicy,
    documents: DocumentPolicy,
    limits: Limits,
) -> Result<String> {
    Ok(sha1_hex(
        serde_json::to_vec(&(content, images, previews, documents, limits))
            .context("encoding source selection policy")?,
    ))
}

pub fn source_policy(source: &Source) -> Result<String> {
    policy(
        source.content,
        source.images,
        source.previews,
        source.documents,
        source.limits,
    )
}

impl Evictions {
    pub fn eligible_first_seen(
        &self,
        current_policy: &str,
    ) -> BTreeMap<String, chrono::DateTime<chrono::Utc>> {
        let mut keys = BTreeMap::new();
        for item in self
            .items
            .values()
            .filter(|item| item.policy != current_policy)
        {
            for key in &item.keys {
                keys.entry(key.clone())
                    .and_modify(|date: &mut chrono::DateTime<chrono::Utc>| {
                        *date = (*date).min(item.first_seen)
                    })
                    .or_insert(item.first_seen);
            }
        }
        keys
    }

    #[cfg(test)]
    pub fn permits(&self, keys: &[String], current_policy: &str) -> bool {
        self.items.values().any(|item| {
            item.policy != current_policy && item.keys.iter().any(|key| keys.contains(key))
        })
    }

    pub fn clear(&mut self, keys: &[String]) -> bool {
        let keys: BTreeSet<_> = keys.iter().map(String::as_str).collect();
        let previous = self.items.len();
        self.items
            .retain(|_, item| !item.keys.iter().any(|key| keys.contains(key.as_str())));
        self.items.len() != previous
    }

    fn record(&mut self, item: &Item, policy: &str) {
        let keys = dedupe_keys(&RawItem {
            title: item.front.title.clone(),
            link: item.front.link.clone(),
            published: item.front.published,
            ..Default::default()
        });
        if !keys.is_empty() {
            self.items.insert(
                item.path.clone(),
                Eviction {
                    keys,
                    policy: policy.into(),
                    first_seen: item.front.first_seen,
                },
            );
        }
    }

    fn validate(&self) -> Result<()> {
        let digest =
            |value: &str| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
        ensure!(
            self.items.values().all(|item| !item.keys.is_empty()
                && item.keys.len() <= 3
                && item.keys.iter().all(|key| digest(key))
                && digest(&item.policy)),
            "invalid policy eviction metadata"
        );
        Ok(())
    }
}

impl Store {
    pub fn evictions(&self, slug: &str) -> Result<Evictions> {
        let relative = Path::new("sources").join(slug).join("evicted.json");
        let path = self.checked_path(&relative)?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Evictions::default());
            }
            Err(error) => return Err(error).context("reading policy evictions"),
        };
        let evictions: Evictions =
            serde_json::from_slice(&bytes).context("parsing policy evictions")?;
        evictions.validate()?;
        Ok(evictions)
    }

    pub fn save_evictions(&self, slug: &str, evictions: &Evictions) -> Result<()> {
        evictions.validate()?;
        let relative = Path::new("sources").join(slug).join("evicted.json");
        let path = self.checked_path(&relative)?;
        if evictions.items.is_empty() {
            return match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error).context("removing empty policy evictions"),
            };
        }
        let text = serde_json::to_string(evictions).context("encoding policy evictions")? + "\n";
        if fs::read_to_string(&path).ok().as_deref() == Some(&text) {
            return Ok(());
        }
        self.write_text(&relative, &text)
    }

    pub fn record_evictions(
        &self,
        items: &[Item],
        paths: &[String],
        sources: &[Source],
        defaults: &Defaults,
    ) -> Result<()> {
        let paths: BTreeSet<_> = paths.iter().map(String::as_str).collect();
        let mut by_source: BTreeMap<&str, Vec<&Item>> = BTreeMap::new();
        for item in items
            .iter()
            .filter(|item| paths.contains(item.path.as_str()))
        {
            by_source.entry(&item.front.source).or_default().push(item);
        }
        for (slug, items) in by_source {
            let policy = if let Some(source) = sources.iter().find(|source| source.slug == slug) {
                source_policy(source)?
            } else {
                policy(
                    defaults.content,
                    defaults.media.images(),
                    defaults.media.previews(),
                    defaults.media.documents(),
                    defaults.limits,
                )?
            };
            let mut evictions = self.evictions(slug)?;
            for item in items {
                evictions.record(item, &policy);
            }
            self.save_evictions(slug, &evictions)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FrontMatter;

    #[test]
    fn only_policy_changes_readmit_evicted_keys_and_restoration_clears_the_marker() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path());
        let mut source = crate::commands::fetch::tests::source();
        let item = Item {
            path: "items/blog/old".into(),
            front: FrontMatter {
                title: "Old article".into(),
                link: "https://example.com/old".into(),
                source: "blog".into(),
                ..Default::default()
            },
            body: "Private article body that must never enter the marker".into(),
        };
        let keys = dedupe_keys(&RawItem {
            link: item.front.link.clone(),
            ..Default::default()
        });
        let original = source_policy(&source).unwrap();
        store
            .record_evictions(
                std::slice::from_ref(&item),
                std::slice::from_ref(&item.path),
                std::slice::from_ref(&source),
                &Defaults::default(),
            )
            .unwrap();
        let path = root.path().join("sources/blog/evicted.json");
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        store
            .record_evictions(
                std::slice::from_ref(&item),
                std::slice::from_ref(&item.path),
                std::slice::from_ref(&source),
                &Defaults::default(),
            )
            .unwrap();
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
        assert!(!fs::read_to_string(&path).unwrap().contains(&item.body));
        let mut evictions = store.evictions("blog").unwrap();
        assert!(!evictions.permits(&keys, &original));
        source.limits.max_items += 1;
        let widened = source_policy(&source).unwrap();
        assert!(evictions.permits(&keys, &widened));
        assert!(!evictions.permits(&[sha1_hex("manually deleted")], &widened));
        assert!(evictions.clear(&keys));
        store.save_evictions("blog", &evictions).unwrap();
        assert!(!path.exists());
        assert!(!store.evictions("blog").unwrap().permits(&keys, &widened));
        store
            .save_evictions("never-created", &Evictions::default())
            .unwrap();
        assert!(!root.path().join("sources/never-created").exists());
    }
}
