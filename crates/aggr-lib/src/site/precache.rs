//! The small install shell and its eager client dependencies. Optional features stay lazy.

use std::collections::BTreeSet;

use super::{assets, render::Renderer};

pub(super) fn paths(renderer: &Renderer, assets: &[String]) -> Vec<String> {
    let mut paths = assets::precache_paths("", assets);
    paths.extend(renderer.eager_assets());
    let mut seen = BTreeSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    paths
}
