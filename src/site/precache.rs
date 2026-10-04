//! The small install shell and its eager client dependencies. Optional features stay lazy.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context as _, Result};

use super::{
    assets,
    render::{Renderer, Theme},
};

fn module_paths(manifest: &BTreeMap<String, serde_json::Value>) -> Vec<String> {
    let mut pending = vec!["src/main.ts".to_string()];
    let mut visited = BTreeSet::new();
    let mut paths = BTreeSet::new();
    while let Some(key) = pending.pop() {
        if !visited.insert(key.clone()) {
            continue;
        }
        let Some(chunk) = manifest.get(&key) else {
            continue;
        };
        if let Some(file) = chunk["file"].as_str() {
            paths.insert(format!("assets/app/{file}"));
        }
        for css in chunk["css"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
        {
            paths.insert(format!("assets/app/{css}"));
        }
        pending.extend(
            chunk["imports"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
                .map(str::to_owned),
        );
    }
    paths.into_iter().collect()
}

pub(super) fn paths(theme: &Theme, renderer: &Renderer, assets: &[String]) -> Result<Vec<String>> {
    let mut paths = assets::precache_paths("", assets);
    if let Some(bytes) = theme.read("static", "app/.vite/manifest.json")? {
        let manifest = serde_json::from_slice(&bytes).context("reading precache module graph")?;
        paths.extend(module_paths(&manifest));
        for entry in ["assets/app/bootstrap.js", "assets/app/worker.js"] {
            paths.push(renderer.asset_site_path(entry));
        }
    }
    let mut seen = BTreeSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_transitive_eager_imports_without_downloading_optional_features() {
        let manifest = serde_json::from_value(serde_json::json!({
            "src/main.ts": {"file":"app.js", "imports":["shared"], "dynamicImports":["search"]},
            "shared": {"file":"shared.js", "imports":["nested"], "css":["shared.css"]},
            "nested": {"file":"nested.js", "imports":["shared"]},
            "search": {"file":"search.js", "css":["search.css"]}
        }))
        .unwrap();
        assert_eq!(
            module_paths(&manifest),
            vec![
                "assets/app/app.js",
                "assets/app/nested.js",
                "assets/app/shared.css",
                "assets/app/shared.js"
            ]
        );
    }
}
