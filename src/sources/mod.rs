//! Source engines: each turns a configured [`Source`] into raw items, using the shared HTTP
//! client and the validators remembered from the previous run.

pub mod aggr;
pub mod feed;
pub mod html;

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
}

/// What the upstream says about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceMeta {
    pub title: Option<String>,
    pub site_url: Option<String>,
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
    match &source.engine {
        Engine::Feed { url } => feed::fetch(url, source, ctx).await,
        Engine::Aggr {
            url,
            branch,
            sources,
            limit,
        } => aggr::fetch(url, branch, sources, *limit, source, ctx).await,
        Engine::Html { url, fields } => html::fetch(url, fields, source, ctx).await,
    }
}

impl Validators {
    pub fn from_state(state: &SourceState) -> Self {
        Self {
            etag: state.etag.clone(),
            last_modified: state.last_modified.clone(),
            body_hash: state.body_hash.clone(),
        }
    }

    pub fn apply(&self, state: &mut SourceState) {
        state.etag = self.etag.clone();
        state.last_modified = self.last_modified.clone();
        state.body_hash = self.body_hash.clone();
    }
}
