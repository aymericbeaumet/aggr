//! The archive directory: source, category and tag collections derived from the retained items,
//! and the syndicated feeds every collection publishes next to its pages.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};

use super::context::{BuildCtx, CategoryCtx, ItemCtx, SiteCtx, SourceCtx, SourceErrorCtx};
use super::{context, display, outputs, write};
use crate::config::Source;
use crate::content;
use crate::model::Item;
use crate::store::{Status, Store};

pub(super) fn source_contexts(
    sources: &[Source],
    store: &Store,
    status: &Status,
    items: &[Item],
) -> Result<Vec<SourceCtx>> {
    let mut counts: BTreeMap<&str, (usize, Option<DateTime<Utc>>)> = BTreeMap::new();
    for item in items {
        let entry = counts.entry(item.front.source.as_str()).or_default();
        entry.0 += 1;
        entry.1 = entry.1.max(Some(item.created_at()));
    }
    let mut contexts = sources
        .iter()
        .map(|source| {
            let state = store.source_state(&source.slug)?;
            let (count, latest) = counts.remove(source.slug.as_str()).unwrap_or_default();
            let name = source
                .name
                .clone()
                .or_else(|| state.title.clone())
                .unwrap_or_else(|| source.slug.clone());
            let fallback = source
                .public_url
                .as_deref()
                .or(state.site_url.as_deref())
                .map(context::domain_of)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "Unknown source".into());
            let name = display::title(&name, &fallback);
            Ok(SourceCtx {
                page: format!("sources/{}/", source.slug),
                slug: source.slug.clone(),
                name,
                url: source.public_url.clone(),
                feed_url: public_http_url(state.resolved_url.as_deref()),
                site_url: state.site_url.clone(),
                language: state.language.clone(),
                category: source
                    .category
                    .as_deref()
                    .and_then(crate::model::normalize_category),
                engine: source.engine.name().to_string(),
                count,
                latest,
                error: status.errors.get(&source.slug).map(|error| SourceErrorCtx {
                    message: error.message.clone(),
                    since: error.since,
                }),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    for (slug, (count, latest)) in counts {
        let state = store.source_state(slug)?;
        let site_url = public_http_url(state.site_url.as_deref());
        let feed_url = public_http_url(state.resolved_url.as_deref());
        let url = feed_url.clone().or_else(|| site_url.clone());
        let fallback = url
            .as_deref()
            .map(context::domain_of)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Unknown source".into());
        let name = display::title(state.title.as_deref().unwrap_or(&fallback), &fallback);
        contexts.push(SourceCtx {
            page: format!("sources/{slug}/"),
            slug: slug.to_string(),
            name,
            url,
            feed_url,
            site_url,
            language: state.language.clone(),
            category: None,
            engine: "web".into(),
            count,
            latest,
            error: None,
        });
    }
    contexts.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.slug.cmp(&b.slug))
    });
    Ok(contexts)
}

/// A stored endpoint reduced to what may appear on a public page: HTTP(S) only, without
/// credentials or sensitive query values.
fn public_http_url(value: Option<&str>) -> Option<String> {
    value
        .and_then(|value| url::Url::parse(value).ok())
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| crate::config::public_url(&url, false))
}

#[derive(Clone, Copy)]
pub(super) enum Taxonomy {
    Categories,
    Tags,
}

pub(super) struct TaxonomyIndex {
    pub(super) terms: Vec<CategoryCtx>,
    pub(super) members: BTreeMap<String, Vec<usize>>,
}

pub(super) fn source_members(items: &[ItemCtx]) -> BTreeMap<String, Vec<usize>> {
    let mut members: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, item) in items.iter().enumerate() {
        members.entry(item.source.clone()).or_default().push(index);
    }
    members
}

pub(super) fn taxonomy_index(items: &[ItemCtx], taxonomy: Taxonomy) -> TaxonomyIndex {
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut members: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, item) in items.iter().enumerate() {
        let terms_for_item: Vec<&str> = match taxonomy {
            Taxonomy::Categories => item.category.as_deref().into_iter().collect(),
            Taxonomy::Tags => item.labels.iter().map(String::as_str).collect(),
        };
        let mut seen = std::collections::BTreeSet::new();
        for name in terms_for_item {
            let slug = context::category_slug(name);
            if slug.is_empty() || !seen.insert(slug.clone()) {
                continue;
            }
            names
                .entry(slug.clone())
                .or_insert_with(|| name.to_string());
            members.entry(slug).or_default().push(index);
        }
    }
    let root = match taxonomy {
        Taxonomy::Categories => "categories",
        Taxonomy::Tags => "tags",
    };
    let mut terms: Vec<_> = names
        .into_iter()
        .map(|(slug, name)| CategoryCtx {
            page: format!("{root}/{slug}/"),
            count: members.get(&slug).map_or(0, Vec::len),
            latest: members
                .get(&slug)
                .into_iter()
                .flatten()
                .map(|&index| items[index].date)
                .max(),
            name,
            slug,
        })
        .collect();
    terms.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.slug.cmp(&b.slug))
    });
    TaxonomyIndex { terms, members }
}

pub(super) fn indexed_items(items: &[ItemCtx], indices: Option<&[usize]>) -> Vec<ItemCtx> {
    indices
        .into_iter()
        .flatten()
        .map(|&index| items[index].clone())
        .collect()
}

/// Prepare a bounded feed once, then hand the same identity/order/content to every serializer.
/// Article rendering is shared across every source, category, tag, and portable representation.
pub(super) fn feed_items(
    items: &[ItemCtx],
    prepared_by_path: &BTreeMap<&str, &content::PreparedMarkdown>,
    limit: usize,
) -> Vec<ItemCtx> {
    items
        .iter()
        .take(limit)
        .cloned()
        .map(|mut item| {
            item.body_html = prepared_by_path
                .get(item.path.as_str())
                .map(|prepared| prepared.portable_html().to_string());
            item
        })
        .collect()
}

pub(super) fn write_collection_feeds(
    out: &Path,
    site: &SiteCtx,
    build: &BuildCtx,
    title: &str,
    path: &str,
    items: &[ItemCtx],
) -> Result<()> {
    let dir = out.join(path);
    write(
        &dir.join("atom.xml"),
        outputs::atom_collection(site, build, title, path, items).as_bytes(),
    )?;
    write(
        &dir.join("rss.xml"),
        outputs::rss_collection(site, build, title, path, items).as_bytes(),
    )?;
    write(
        &dir.join("feed.json"),
        outputs::json_collection(site, title, path, items)?.as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_without_rendered_items_are_omitted() {
        assert!(taxonomy_index(&[], Taxonomy::Categories).terms.is_empty());
    }
}
