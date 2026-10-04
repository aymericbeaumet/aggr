//! Complete offline article families and an integrity-checked search index manifest.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context as _, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{assets, context::ItemCtx, pagefind::SearchCatalogue};
use crate::content;

#[derive(Serialize)]
pub(super) struct Article {
    url: String,
    title: String,
    resources: Vec<assets::PrecacheEntry>,
}

pub(super) fn catalogue(
    out: &Path,
    items: &[ItemCtx],
    images: &BTreeMap<String, Vec<content::LocalImage>>,
    published: &assets::Published,
) -> Result<Vec<Article>> {
    let families: Vec<_> = items
        .iter()
        .take(1000)
        .map(|item| {
            let mut paths = BTreeSet::from([item.url.clone()]);
            // A remote preview URL is a publisher address, not a file this build wrote.
            if let Some(preview) = &item.preview
                && published.contains_key(&preview.url)
            {
                paths.insert(preview.url.clone());
            }
            if let Some(preview) = &item.article_preview
                && published.contains_key(&preview.url)
            {
                paths.insert(preview.url.clone());
            }
            if let Some(local) = item
                .document
                .as_ref()
                .and_then(|document| document.local_url.as_deref())
            {
                paths.insert(local.split('#').next().unwrap_or(local).to_owned());
            }
            for image in images.get(&item.path).into_iter().flatten() {
                if published.contains_key(&image.original) {
                    paths.insert(image.original.clone());
                }
                paths.extend(
                    image
                        .variants
                        .iter()
                        .map(|variant| variant.url.clone())
                        .filter(|url| published.contains_key(url)),
                );
            }
            (item, paths)
        })
        .collect();
    let paths = families
        .iter()
        .flat_map(|(_, paths)| paths.iter().cloned())
        .collect::<BTreeSet<_>>();
    let revisions: BTreeMap<_, _> =
        assets::precache_entries(out, paths.into_iter().collect(), published)?
            .into_iter()
            .map(|entry| (entry.url.clone(), entry))
            .collect();
    Ok(families
        .into_iter()
        .map(|(item, paths)| Article {
            url: item.url.clone(),
            title: item.title.clone(),
            resources: paths
                .into_iter()
                .filter_map(|path| revisions.get(&path).cloned())
                .collect(),
        })
        .collect())
}

/// Publish the vocabulary with its index, so offline queries cannot mix different builds.
pub(super) fn search_manifest(
    out: &Path,
    catalogue: &SearchCatalogue,
) -> Result<serde_json::Value> {
    let directory = out.join(&catalogue.base);
    crate::cache::write(
        &directory.join("search-catalog.json"),
        &serde_json::to_vec(catalogue)?,
    )?;
    let mut files = Vec::new();
    let mut total = 0_u64;
    for entry in walkdir::WalkDir::new(&directory).sort_by_file_name() {
        let entry = entry.context("enumerating offline search resources")?;
        if !entry.file_type().is_file() || entry.file_name() == "search-manifest.json" {
            continue;
        }
        let bytes = std::fs::read(entry.path()).context("reading offline search resource")?;
        let size = bytes.len() as u64;
        total += size;
        files.push(serde_json::json!({
            "url": entry.path().strip_prefix(out)?.to_string_lossy().replace('\\', "/"),
            "size": size,
            "digest": hex::encode(Sha256::digest(&bytes)),
        }));
    }
    let manifest = serde_json::json!({"version": catalogue.version, "base": catalogue.base, "files": files, "totalBytes": total});
    let bytes = serde_json::to_vec(&manifest)?;
    crate::cache::write(&directory.join("search-manifest.json"), &bytes)?;
    crate::cache::write(&out.join("search-manifest.json"), &bytes)?;
    Ok(serde_json::json!({"version": catalogue.version, "base": catalogue.base}))
}
