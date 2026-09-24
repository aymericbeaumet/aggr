//! Source engines: each turns a configured [`Source`] into raw items, using the shared HTTP
//! client and the validators remembered from the previous run.

pub mod aggr;
pub mod feed;
pub mod html;
pub mod instagram;
pub mod podcast;
pub mod qwen;
pub mod youtube;

use std::path::Path;

use anyhow::Result;

use crate::config::{Engine, Source};
use crate::http;
use crate::model::RawItem;
use crate::store::SourceState;

pub struct Context<'a> {
    pub client: &'a http::Client,
    pub state: &'a SourceState,
    /// Scratch space kept between runs (`.aggr/cache`): mirrors of other aggr repositories.
    pub cache_dir: &'a Path,
}

/// HTTP validators and a body hash: enough to skip parsing when nothing moved upstream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub body_hash: Option<String>,
    /// Actual endpoint used after feed discovery or redirects.
    pub resolved_url: Option<String>,
}

/// What the upstream says about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMeta {
    pub title: Option<String>,
    pub site_url: Option<String>,
    /// BCP 47 tag the publisher declares for the whole source (RSS `<language>`, Atom `xml:lang`,
    /// JSON Feed `language`, or the page's `<html lang>`), already canonicalised by
    /// [`normalize_language`]. Items inherit it at build time, so the source-level value covers
    /// articles archived long before it was learned.
    pub language: Option<String>,
    /// Whether the items were read off a page rather than parsed from a feed. A publisher that
    /// retires its feed keeps answering at the same URL, and a scrape of the page left behind
    /// looks healthy until someone is told which it was.
    pub extracted: bool,
}

/// Canonical form of a publisher-declared language tag, or `None` when it is not a well-formed
/// BCP 47 tag (`en_US`, `123`, free text). Tags compare case-insensitively and publishers and
/// parsers disagree on case (`fr-fr`, `EN-gb`), so the RFC 5646 §2.1.1 conventions are applied:
/// every subtag is lowercased except a two-letter region, which is uppercased (`en-GB`), and a
/// four-letter script, which is title-cased (`zh-Hant-TW`); extension and private-use subtags,
/// everything after the first singleton, stay lowercase (`en-US-u-ca-gregory`).
pub fn normalize_language(tag: &str) -> Option<String> {
    let tag = tag.trim();
    if !crate::config::language::is_well_formed(tag) {
        return None;
    }
    let mut after_singleton = false;
    let subtags = tag
        .split('-')
        .enumerate()
        .map(|(index, subtag)| {
            let alphabetic = subtag.bytes().all(|byte| byte.is_ascii_alphabetic());
            let cased = match subtag.len() {
                2 if index > 0 && !after_singleton && alphabetic => subtag.to_ascii_uppercase(),
                4 if index > 0 && !after_singleton && alphabetic => {
                    let (head, tail) = subtag.split_at(1);
                    head.to_ascii_uppercase() + &tail.to_ascii_lowercase()
                }
                _ => subtag.to_ascii_lowercase(),
            };
            after_singleton |= subtag.len() == 1;
            cased
        })
        .collect::<Vec<_>>();
    Some(subtags.join("-"))
}

pub enum Fetch {
    /// 304, or a body identical to last time. Validators may still be newer than stored.
    Unchanged { validators: Validators },
    Changed {
        validators: Validators,
        meta: SourceMeta,
        items: Vec<RawItem>,
    },
}

pub async fn fetch(source: &Source, ctx: &Context<'_>) -> Result<Fetch> {
    let mut fetched = match &source.engine {
        Engine::Feed { url } if instagram::is_profile_url(url) => {
            instagram::fetch(url, source, ctx).await
        }
        Engine::Feed { url } if qwen::is_blog_url(url) => qwen::fetch(url, source, ctx).await,
        Engine::Feed { url } if podcast::is_show_url(url) => podcast::fetch(url, source, ctx).await,
        Engine::Feed { url } => feed::fetch(url, source, ctx).await,
        Engine::Aggr {
            url,
            branch,
            sources,
            limit,
        } => aggr::fetch(url, branch, sources, *limit, source, ctx).await,
    }?;
    if let Fetch::Changed { items, .. } = &mut fetched {
        items.retain(|item| {
            !url::Url::parse(&item.link).is_ok_and(|url| youtube::is_short_url(&url))
        });
    }
    Ok(fetched)
}

impl Validators {
    pub fn from_state(state: &SourceState) -> Self {
        Self {
            etag: state.etag.clone(),
            last_modified: state.last_modified.clone(),
            body_hash: state.body_hash.clone(),
            resolved_url: state.resolved_url.clone(),
        }
    }

    pub fn apply(&self, state: &mut SourceState) {
        state.etag = self.etag.clone();
        state.last_modified = self.last_modified.clone();
        state.body_hash = self.body_hash.clone();
        state.resolved_url = self.resolved_url.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_language;

    #[test]
    fn declared_languages_are_canonicalised_and_garbage_is_dropped() {
        for (declared, expected) in [
            ("en", "en"),
            ("EN", "en"),
            ("EN-gb", "en-GB"),
            ("fr-fr", "fr-FR"),
            (" fr-FR\n", "fr-FR"),
            ("zh-hant-tw", "zh-Hant-TW"),
            ("ZH-CMN-HANS-CN", "zh-cmn-Hans-CN"),
            ("de-CH-1901", "de-CH-1901"),
            ("en-US-u-CA-Gregory", "en-US-u-ca-gregory"),
            ("EN-CA-X-CA", "en-CA-x-ca"),
            ("X-Reader-Local", "x-reader-local"),
            ("I-Klingon", "i-klingon"),
            ("sgn-be-fr", "sgn-BE-FR"),
        ] {
            assert_eq!(
                normalize_language(declared).as_deref(),
                Some(expected),
                "{declared:?}"
            );
        }
        for declared in [
            "",
            "   ",
            "e",
            "en_US",
            "en-",
            "123",
            "fr-É",
            "English (US)",
            "x",
        ] {
            assert_eq!(normalize_language(declared), None, "{declared:?}");
        }
    }
}
