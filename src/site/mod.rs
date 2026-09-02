//! Static site generation: the data tree + config + git facts in, a directory of files out.
//! The planning half (`plan`) is pure; `build` does the IO.

pub mod context;
pub mod outputs;
mod pagefind;
pub mod render;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use crate::config::{Config, Source};
use crate::content;
use crate::model::Item;
use crate::store::{Status, Store};
use context::{
    BuildCtx, CategoryCtx, GitHubLinks, ItemCtx, PageCtx, SiteCtx, SourceCtx, SourceErrorCtx,
};
use render::{Layers, Renderer};

const EXCERPT_CHARS: usize = 240;
const MARKER: &str = ".aggr-site";

/// Facts about the build that do not come from the data tree.
pub struct BuildInfo {
    pub out: PathBuf,
    pub base_url: Option<String>,
    pub config_sha: Option<String>,
    /// Repository-relative path of the root config file.
    pub config_path: Option<String>,
    pub data_sha: Option<String>,
    pub now: DateTime<Utc>,
    /// Production build: absolute URLs from `base_url`, CNAME for custom domains.
    pub release: bool,
}

/// Which items get full pages and which get redirect stubs.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Window {
    pub rendered: Vec<usize>,
    pub stubbed: Vec<usize>,
}

/// Split items (sorted newest first, hidden already removed) into the site window and the rest.
pub fn window(
    dates: &[DateTime<Utc>],
    now: DateTime<Utc>,
    max_items: usize,
    max_age_days: u32,
    max_stubs: usize,
) -> Window {
    let cutoff = now - Duration::days(i64::from(max_age_days));
    let mut out = Window::default();
    for (index, date) in dates.iter().enumerate() {
        if out.rendered.len() < max_items && *date >= cutoff {
            out.rendered.push(index);
        } else if out.stubbed.len() < max_stubs {
            out.stubbed.push(index);
        }
    }
    out
}

/// Page paths for `total` items at `per_page`, first page at `prefix`, then `prefix + page/N/`.
pub fn paginate(
    prefix: &str,
    total: usize,
    per_page: usize,
) -> Vec<(usize, String, std::ops::Range<usize>)> {
    let pages = total.div_ceil(per_page).max(1);
    (1..=pages)
        .map(|number| {
            let path = if number == 1 {
                prefix.to_string()
            } else {
                format!("{prefix}page/{number}/")
            };
            let start = (number - 1) * per_page;
            (number, path, start..(start + per_page).min(total))
        })
        .collect()
}

/// Everything the service worker fetches at install, as site paths under `base`: the shells,
/// the first page of every list, every asset and the newest `offline_items` item pages.
pub fn precache_paths(
    base: &str,
    lists: impl IntoIterator<Item = String>,
    assets: &[String],
    item_urls: impl IntoIterator<Item = String>,
    offline_items: usize,
) -> Vec<String> {
    const SHELLS: [&str; 7] = [
        "",
        "sources/",
        "search/",
        "settings/",
        "404.html",
        "offline.html",
        "manifest.webmanifest",
    ];
    SHELLS
        .iter()
        .map(|path| path.to_string())
        .chain(lists)
        .chain(assets.iter().map(|name| format!("assets/{name}")))
        .chain(item_urls.into_iter().take(offline_items))
        .map(|path| format!("{base}{path}"))
        .collect()
}

/// Names the precache. Every build gets its own so a page is never served from the previous
/// build's cache once the new worker has taken over.
pub fn cache_version(build: &BuildCtx) -> String {
    fn short(sha: Option<&str>) -> &str {
        sha.map_or("local", |sha| &sha[..sha.len().min(12)])
    }
    format!(
        "{}-{}-{}",
        short(build.data_sha.as_deref()),
        short(build.config_sha.as_deref()),
        build.time.format("%Y%m%d%H%M%S")
    )
}

/// `https://user.github.io/repo/` → `/repo/`; anything unparsable → `/`.
pub fn base_path(base_url: Option<&str>) -> String {
    let path = base_url
        .and_then(|url| url::Url::parse(url).ok())
        .map(|url| url.path().to_string())
        .unwrap_or_default();
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        format!("/{trimmed}/")
    }
}

/// Relative reference from a generated page back to the output root. Directory routes end in
/// `/`; standalone root files such as `offline.html` do not add a level.
pub fn relative_root(path: &str) -> String {
    let depth = path
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .count()
        .saturating_sub(usize::from(!path.is_empty() && !path.ends_with('/')));
    if depth == 0 {
        "./".into()
    } else {
        "../".repeat(depth)
    }
}

#[derive(Serialize)]
struct Ctx<'a> {
    site: &'a SiteCtx,
    build: &'a BuildCtx,
    page: PageCtx,
    items: &'a [ItemCtx],
    sources: &'a [SourceCtx],
    categories: &'a [CategoryCtx],
    tags: &'a [CategoryCtx],
    #[serde(skip_serializing_if = "Option::is_none")]
    item: Option<&'a ItemCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a SourceCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<&'a CategoryCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    html: Option<&'a str>,
}

/// What `sw.js` sees: the cache name and the install-time fetch list.
#[derive(Serialize)]
struct SwCtx<'a> {
    site: &'a SiteCtx,
    build: &'a BuildCtx,
    version: String,
    precache: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub pages: usize,
    pub items: usize,
    pub stubs: usize,
}

pub fn build(
    config: &Config,
    sources: &[Source],
    store: &Store,
    project_root: &Path,
    info: &BuildInfo,
) -> Result<Summary> {
    let out = &info.out;
    prepare_out_dir(out)?;
    let config_name = info
        .config_path
        .as_deref()
        .and_then(|path| Path::new(path).file_name())
        .unwrap_or_else(|| std::ffi::OsStr::new("aggr.toml"));
    let config_source = project_root.join(config_name);
    if config_source.is_file() {
        write(&out.join("aggr.toml"), &std::fs::read(&config_source)?)?;
    }

    let base = base_path(info.base_url.as_deref());
    let repository = config.repository();
    let site = SiteCtx {
        title: config.site.title.clone(),
        description: config.site.description.clone(),
        base_path: base.clone(),
        base_url: info.base_url.clone().map(|url| ensure_trailing_slash(&url)),
        repository: repository.clone(),
        data_branch: config.store.branch.clone(),
        pwa: config.site.pwa,
        config_url: repository.as_deref().and_then(|repository| {
            info.config_sha.as_deref().map(|sha| {
                format!(
                    "https://raw.githubusercontent.com/{repository}/{sha}/{}",
                    info.config_path.as_deref().unwrap_or("aggr.toml")
                )
            })
        }),
        discussions: config
            .site
            .discussions
            .iter()
            .map(|d| context::DiscussionLinkCtx {
                name: context::compact_name(&d.name),
                url: d.url.clone(),
            })
            .collect(),
        params: config.site.params.clone(),
    };
    let build_ctx = BuildCtx {
        time: info.now,
        version: env!("CARGO_PKG_VERSION").to_string(),
        config_sha: info.config_sha.clone(),
        data_sha: info.data_sha.clone(),
        release: info.release,
    };
    let links = repository.as_deref().map(|repository| GitHubLinks {
        repository,
        branch: &config.store.branch,
        data_sha: info.data_sha.as_deref(),
    });

    let renderer = Renderer::new(theme_layers(config, project_root)?, &base)?;

    // Sources: config order, enriched with stored state and counts.
    let status = store.status()?;
    let mut all_items = store.items()?;
    all_items.retain(|item| !item.front.hidden);
    all_items.sort_by(|a, b| {
        b.created_at()
            .cmp(&a.created_at())
            .then_with(|| b.path.cmp(&a.path))
    });
    let source_ctxs = source_contexts(sources, store, &status, &all_items)?;
    let source_by_slug: BTreeMap<&str, &SourceCtx> =
        source_ctxs.iter().map(|s| (s.slug.as_str(), s)).collect();

    let dates: Vec<_> = all_items.iter().map(Item::created_at).collect();
    let window = window(
        &dates,
        info.now,
        config.site.max_items,
        config.site.max_age_days,
        config.site.max_stubs,
    );

    let mut items = Vec::with_capacity(window.rendered.len());
    for &index in &window.rendered {
        let item = &all_items[index];
        let (source_name, category) = source_by_slug
            .get(item.front.source.as_str())
            .map(|s| (s.name.as_str(), s.category.as_deref()))
            .unwrap_or((item.front.source.as_str(), None));
        let excerpt = item
            .front
            .summary
            .clone()
            .map(|s| content::excerpt(&s, EXCERPT_CHARS))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| content::excerpt(&item.body, EXCERPT_CHARS));
        items.push(ItemCtx::from_item(
            item,
            source_name,
            category,
            links.as_ref(),
            excerpt,
            &config.site.discussions,
        ));
    }

    let categories = category_contexts(&source_ctxs, &items);
    let tags = tag_contexts(&items);
    let mut pages = 0;
    let per_page = config.site.items_per_page;
    let mut write_list = |kind: &str,
                          title: &str,
                          prefix: &str,
                          list: &[ItemCtx],
                          source: Option<&SourceCtx>,
                          category: Option<&CategoryCtx>|
     -> Result<()> {
        let pagination = paginate(prefix, list.len(), per_page);
        let total = pagination.len();
        for (number, path, range) in &pagination {
            let page = PageCtx {
                kind: kind.to_string(),
                title: title.to_string(),
                path: path.clone(),
                root: relative_root(path),
                number: *number,
                total,
                offset: range.start,
                prev: (*number > 1).then(|| pagination[number - 2].1.clone()),
                next: (*number < total).then(|| pagination[*number].1.clone()),
            };
            let html = renderer.render(
                "index.html",
                Ctx {
                    site: &site,
                    build: &build_ctx,
                    page,
                    items: &list[range.clone()],
                    sources: &source_ctxs,
                    categories: &categories,
                    tags: &tags,
                    item: None,
                    source,
                    category,
                    html: None,
                },
            )?;
            write(&out.join(path).join("index.html"), html.as_bytes())?;
            pages += 1;
        }
        Ok(())
    };

    write_list("river", &config.site.title, "", &items, None, None)?;
    for source in &source_ctxs {
        let list: Vec<ItemCtx> = items
            .iter()
            .filter(|i| i.source == source.slug)
            .cloned()
            .collect();
        write_list(
            "source",
            &source.name,
            &source.page,
            &list,
            Some(source),
            None,
        )?;
    }
    for category in &categories {
        let list: Vec<ItemCtx> = items
            .iter()
            .filter(|i| i.category.as_deref() == Some(&category.name))
            .cloned()
            .collect();
        write_list(
            "category",
            &category.name,
            &category.page,
            &list,
            None,
            Some(category),
        )?;
        write_collection_feeds(
            out,
            &site,
            &build_ctx,
            &category.name,
            &category.page,
            &list[..list.len().min(per_page)],
        )?;
    }
    for tag in &tags {
        let list: Vec<ItemCtx> = items
            .iter()
            .filter(|item| {
                item.labels
                    .iter()
                    .any(|label| context::category_slug(label) == tag.slug)
            })
            .cloned()
            .collect();
        write_list("tag", &tag.name, &tag.page, &list, None, Some(tag))?;
        write_collection_feeds(
            out,
            &site,
            &build_ctx,
            &tag.name,
            &tag.page,
            &list[..list.len().min(per_page)],
        )?;
    }

    let simple = |kind: &str,
                  title: &str,
                  path: &str,
                  template: &str,
                  item: Option<&ItemCtx>,
                  html: Option<&str>|
     -> Result<String> {
        let page = PageCtx {
            kind: kind.to_string(),
            title: title.to_string(),
            path: path.to_string(),
            root: relative_root(path),
            number: 1,
            total: 1,
            offset: 0,
            prev: None,
            next: None,
        };
        renderer.render(
            template,
            Ctx {
                site: &site,
                build: &build_ctx,
                page,
                items: &items,
                sources: &source_ctxs,
                categories: &categories,
                tags: &tags,
                item,
                source: None,
                category: None,
                html,
            },
        )
    };

    write(
        &out.join("sources/index.html"),
        simple("sources", "Sources", "sources/", "sources.html", None, None)?.as_bytes(),
    )?;
    write(
        &out.join("categories/index.html"),
        simple(
            "categories",
            "Categories",
            "categories/",
            "taxonomy.html",
            None,
            None,
        )?
        .as_bytes(),
    )?;
    write(
        &out.join("tags/index.html"),
        simple("tags", "Tags", "tags/", "taxonomy.html", None, None)?.as_bytes(),
    )?;
    write(
        &out.join("search/index.html"),
        simple("search", "Search", "search/", "shell.html", None, None)?.as_bytes(),
    )?;
    write(
        &out.join("settings/index.html"),
        simple(
            "settings",
            "Settings",
            "settings/",
            "settings.html",
            None,
            None,
        )?
        .as_bytes(),
    )?;
    write(
        &out.join("404.html"),
        simple("404", "Not found", "", "404.html", None, None)?.as_bytes(),
    )?;
    pages += 6;

    for (ctx, &index) in items.iter().zip(&window.rendered) {
        let item = &all_items[index];
        let mut ctx = ctx.clone();
        ctx.body_html = Some(content::render_markdown(&item.body));
        let dir = out.join(&ctx.url);
        let representation = out.join(ctx.url.trim_end_matches('/'));
        write(
            &dir.join("index.html"),
            simple("item", &ctx.title, &ctx.url, "item.html", Some(&ctx), None)?.as_bytes(),
        )?;
        write(
            &representation.with_extension("md"),
            &store.item_bytes(&item.path)?,
        )?;
        write(
            &representation.with_extension("txt"),
            outputs::text_item(&ctx).as_bytes(),
        )?;
        write(
            &representation.with_extension("rst"),
            outputs::rst_item(&ctx).as_bytes(),
        )?;
        write(
            &representation.with_extension("json"),
            outputs::item_json(&site, &ctx, &item.body)?.as_bytes(),
        )?;
    }

    let mut stubs = 0;
    if let Some(links) = &links {
        for &index in &window.stubbed {
            let item = &all_items[index];
            let stub = outputs::redirect_stub(&links.permalink(&item.md_path()));
            write(
                &out.join(context::item_url(&item.path)).join("index.html"),
                stub.as_bytes(),
            )?;
            stubs += 1;
        }
    }

    let feed_items = &items[..items.len().min(per_page)];
    let atom = outputs::atom_feed(&site, &build_ctx, feed_items);
    write(&out.join("feed.xml"), atom.as_bytes())?;
    write(&out.join("atom.xml"), atom.as_bytes())?;
    write(
        &out.join("rss.xml"),
        outputs::rss_collection(&site, &build_ctx, &site.title, "", feed_items).as_bytes(),
    )?;
    write(
        &out.join("feed.json"),
        outputs::json_collection(&site, &site.title, "", feed_items)?.as_bytes(),
    )?;
    write_collection_feeds(
        out,
        &site,
        &build_ctx,
        &format!("{} categories", site.title),
        "categories/",
        feed_items,
    )?;
    write_collection_feeds(
        out,
        &site,
        &build_ctx,
        &format!("{} tags", site.title),
        "tags/",
        feed_items,
    )?;
    write(&out.join(".nojekyll"), b"")?;
    write(&out.join(MARKER), env!("CARGO_PKG_VERSION").as_bytes())?;
    if let Some(domain) = cname(info) {
        write(&out.join("CNAME"), domain.as_bytes())?;
    }
    let assets = renderer.write_static(out)?;

    if config.site.pwa {
        write(
            &out.join("offline.html"),
            simple(
                "offline",
                "Offline",
                "offline.html",
                "offline.html",
                None,
                None,
            )?
            .as_bytes(),
        )?;
        write(
            &out.join("manifest.webmanifest"),
            simple(
                "manifest",
                &config.site.title,
                "manifest.webmanifest",
                "manifest.webmanifest",
                None,
                None,
            )?
            .as_bytes(),
        )?;
        let lists = source_ctxs
            .iter()
            .map(|s| s.page.clone())
            .chain(categories.iter().map(|c| c.page.clone()))
            .chain(tags.iter().map(|tag| tag.page.clone()))
            .chain(["categories/".to_string(), "tags/".to_string()]);
        let sw = renderer.render(
            "sw.js",
            SwCtx {
                site: &site,
                build: &build_ctx,
                version: cache_version(&build_ctx),
                precache: precache_paths(
                    "",
                    lists,
                    &assets,
                    items.iter().map(|i| i.url.clone()),
                    config.site.offline_items,
                ),
            },
        )?;
        write(&out.join("sw.js"), sw.as_bytes())?;
        pages += 1;
    }

    pagefind::build(out)?;

    Ok(Summary {
        pages,
        items: items.len(),
        stubs,
    })
}

/// Template/static lookup order: the project's own `templates/`+`static/`, then the configured
/// theme directory, then the embedded default theme.
pub fn theme_layers(config: &Config, project_root: &Path) -> Result<Layers> {
    let mut layers = Layers::default();
    if project_root.join("templates").is_dir() || project_root.join("static").is_dir() {
        layers.dirs.push(project_root.to_path_buf());
    }
    match config.site.theme.as_str() {
        "default" => {
            // A development binary reads the shipped theme from its source tree so `aggr dev`
            // can rebuild template/CSS/JS edits without recompiling Rust. Release binaries stay
            // fully embedded and have no dependency on the build machine.
            #[cfg(debug_assertions)]
            {
                let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("themes/default");
                if source.is_dir() && !source.starts_with(project_root) {
                    layers.dirs.push(source);
                }
            }
        }
        theme => {
            let dir = project_root.join(theme);
            if !dir.is_dir() {
                bail!("theme {theme:?} is not a directory (git themes arrive in a later release)");
            }
            layers.dirs.push(dir);
        }
    }
    Ok(layers)
}

fn source_contexts(
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
    sources
        .iter()
        .map(|source| {
            let state = store.source_state(&source.slug)?;
            let (count, latest) = counts
                .get(source.slug.as_str())
                .copied()
                .unwrap_or_default();
            let name = source
                .name
                .clone()
                .or_else(|| state.title.clone())
                .unwrap_or_else(|| source.slug.clone());
            Ok(SourceCtx {
                page: format!("sources/{}/", source.slug),
                slug: source.slug.clone(),
                name,
                url: source.engine.url().map(|url| url.to_string()),
                site_url: state.site_url.clone(),
                category: source.category.clone(),
                engine: source.engine.name().to_string(),
                count,
                latest,
                error: status.errors.get(&source.slug).map(|error| SourceErrorCtx {
                    message: error.message.clone(),
                    since: error.since,
                }),
            })
        })
        .collect()
}

fn category_contexts(sources: &[SourceCtx], items: &[ItemCtx]) -> Vec<CategoryCtx> {
    let mut categories: BTreeMap<&str, usize> = sources
        .iter()
        .filter_map(|s| s.category.as_deref())
        .map(|c| (c, 0))
        .collect();
    for item in items {
        if let Some(category) = item.category.as_deref()
            && let Some(count) = categories.get_mut(category)
        {
            *count += 1;
        }
    }
    categories
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(name, count)| {
            let slug = context::category_slug(name);
            CategoryCtx {
                name: name.to_string(),
                page: format!("categories/{slug}/"),
                slug,
                count,
            }
        })
        .collect()
}

fn tag_contexts(items: &[ItemCtx]) -> Vec<CategoryCtx> {
    let mut tags: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for item in items {
        for name in &item.labels {
            let slug = context::category_slug(name);
            let entry = tags.entry(slug).or_insert_with(|| (name.clone(), 0));
            entry.1 += 1;
        }
    }
    tags.into_iter()
        .map(|(slug, (name, count))| CategoryCtx {
            page: format!("tags/{slug}/"),
            name,
            slug,
            count,
        })
        .collect()
}

fn write_collection_feeds(
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

/// Wipe a previous build, refusing to touch a directory we did not create.
pub(crate) fn prepare_out_dir(out: &Path) -> Result<()> {
    if out.exists() {
        let ours = out.join(MARKER).exists() || std::fs::read_dir(out)?.next().is_none();
        if !ours {
            bail!(
                "refusing to clear {}: not an aggr output directory (no {MARKER} marker)",
                out.display()
            );
        }
        std::fs::remove_dir_all(out).with_context(|| format!("clearing {}", out.display()))?;
    }
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))
}

/// The host `CNAME` should carry: a release build on a domain that is not GitHub's own.
pub fn cname(info: &BuildInfo) -> Option<String> {
    if !info.release {
        return None;
    }
    let url = url::Url::parse(info.base_url.as_deref()?).ok()?;
    let host = url.host_str()?;
    (!host.ends_with(".github.io") && host != "localhost").then(|| host.to_string())
}

fn ensure_trailing_slash(url: &str) -> String {
    if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{url}/")
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn day(d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn window_respects_count_age_and_stub_cap() {
        let dates = vec![day(10), day(9), day(8), day(1)];
        let w = window(&dates, day(10), 2, 30, 1);
        assert_eq!(
            w,
            Window {
                rendered: vec![0, 1],
                stubbed: vec![2]
            }
        );
        let w = window(&dates, day(10), 10, 5, 10);
        assert_eq!(
            w,
            Window {
                rendered: vec![0, 1, 2],
                stubbed: vec![3]
            }
        );
    }

    #[test]
    fn pagination_paths() {
        let pages = paginate("", 0, 10);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].1, "");
        let pages = paginate("sources/x/", 25, 10);
        assert_eq!(
            pages.iter().map(|p| p.1.as_str()).collect::<Vec<_>>(),
            ["sources/x/", "sources/x/page/2/", "sources/x/page/3/"]
        );
        assert_eq!(pages[2].2, 20..25);
    }

    #[test]
    fn categories_without_rendered_items_are_omitted() {
        let source = SourceCtx {
            slug: "empty".into(),
            name: "Empty".into(),
            url: None,
            site_url: None,
            category: Some("unused".into()),
            engine: "web".into(),
            count: 0,
            latest: None,
            error: None,
            page: "sources/empty/".into(),
        };
        assert!(category_contexts(&[source], &[]).is_empty());
    }

    #[test]
    fn base_paths() {
        assert_eq!(base_path(None), "/");
        assert_eq!(base_path(Some("https://u.github.io/")), "/");
        assert_eq!(base_path(Some("https://u.github.io/repo")), "/repo/");
        assert_eq!(base_path(Some("https://u.github.io/a/b/")), "/a/b/");
    }

    #[test]
    fn relative_roots_cover_files_and_nested_routes() {
        assert_eq!(relative_root(""), "./");
        assert_eq!(relative_root("404.html"), "./");
        assert_eq!(relative_root("search/"), "../");
        assert_eq!(relative_root("categories/rust/"), "../../");
        assert_eq!(relative_root("sources/rust/page/2/"), "../../../../");
    }

    #[test]
    fn refuses_to_clear_foreign_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("precious"), "x").unwrap();
        assert!(prepare_out_dir(dir.path()).is_err());
        std::fs::write(dir.path().join(MARKER), "").unwrap();
        prepare_out_dir(dir.path()).unwrap();
        assert!(!dir.path().join("precious").exists());
    }

    #[test]
    fn precache_lists_shells_lists_assets_and_the_newest_items() {
        let paths = precache_paths(
            "/repo/",
            ["sources/a/".to_string()],
            &["style.css".to_string()],
            ["items/a/1/".to_string(), "items/a/2/".to_string()],
            1,
        );
        assert_eq!(paths[0], "/repo/");
        assert!(paths.contains(&"/repo/offline.html".to_string()));
        assert!(paths.contains(&"/repo/settings/".to_string()));
        assert!(paths.contains(&"/repo/sources/a/".to_string()));
        assert!(paths.contains(&"/repo/assets/style.css".to_string()));
        assert!(paths.contains(&"/repo/items/a/1/".to_string()));
        assert!(!paths.contains(&"/repo/items/a/2/".to_string()));
    }

    #[test]
    fn cache_version_changes_with_data_config_and_time() {
        let build = BuildCtx {
            time: day(2),
            version: "0".into(),
            config_sha: Some("c".repeat(40)),
            data_sha: None,
            release: false,
        };
        assert_eq!(
            cache_version(&build),
            format!("local-{}-20260902000000", "c".repeat(12))
        );
        let later = BuildCtx {
            data_sha: Some("d".repeat(40)),
            time: day(3),
            ..build.clone()
        };
        assert_ne!(cache_version(&later), cache_version(&build));
    }

    /// A store with `count` items of one source, newest last, and a matching config.
    fn fixture(root: &Path, count: usize, extra: &str) -> (Config, Vec<Source>, Store) {
        use crate::model::{FrontMatter, file_stem, item_dir};
        use crate::store::NewItem;

        let config = Config::parse(&format!(
            "[site]\ntitle = \"Demo <site>\"\n{extra}\n[[sources]]\nslug = \"blog\"\nurl = \"https://blog.example/feed\"\n"
        ))
        .unwrap();
        let sources = config.resolve_sources(&|_| None).unwrap();
        let store = Store::open(root.join("data"));
        for i in 0..count {
            let date = day(1 + i as u32);
            let front = FrontMatter {
                title: format!("Post {i}"),
                link: format!("https://blog.example/{i}"),
                source: "blog".into(),
                published: Some(date),
                first_seen: date,
                ..Default::default()
            };
            store
                .write_item(NewItem {
                    dir: &item_dir("blog", date),
                    stem: &file_stem(date, &front.title),
                    front: &front,
                    body: "Hello",
                    html: None,
                })
                .unwrap();
        }
        (config, sources, store)
    }

    fn info(out: PathBuf) -> BuildInfo {
        BuildInfo {
            out,
            base_url: Some("https://u.github.io/repo/".into()),
            config_sha: Some("c".repeat(40)),
            config_path: Some("aggr.toml".into()),
            data_sha: Some("d".repeat(40)),
            now: day(20),
            release: false,
        }
    }

    #[test]
    fn pwa_outputs_cover_the_shells_and_the_newest_items() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 3, "offline_items = 2\n");
        let out = dir.path().join("out");
        let summary = build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        assert_eq!(summary.items, 3);
        // River, the source page, the six fixed pages, the offline page.
        assert_eq!(summary.pages, 1 + 1 + 6 + 1);

        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("manifest.webmanifest")).unwrap())
                .unwrap();
        assert_eq!(manifest["name"], "Demo <site>");
        assert_eq!(manifest["start_url"], "./");
        assert_eq!(manifest["scope"], "./");
        assert_eq!(manifest["display"], "standalone");
        assert_eq!(manifest["icons"].as_array().unwrap().len(), 4);
        assert!(
            manifest["icons"][0]["src"]
                .as_str()
                .unwrap()
                .starts_with("assets/icon-192-")
        );
        for icon in [
            "icon-192-",
            "icon-512-",
            "icon-maskable-512-",
            "apple-touch-icon-",
        ] {
            let path = std::fs::read_dir(out.join("assets"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(icon)
                })
                .unwrap();
            let png = std::fs::read(path).unwrap();
            assert!(png.starts_with(b"\x89PNG"), "{icon} is not a PNG");
        }

        let sw = std::fs::read_to_string(out.join("sw.js")).unwrap();
        let version = format!("{}-{}-20260920000000", "d".repeat(12), "c".repeat(12));
        assert!(sw.contains(&format!("var VERSION = {version:?};")), "{sw}");
        assert!(sw.contains("new URL(\"./\", self.registration.scope)"));
        assert!(sw.contains("\"assets/style-"));
        assert!(sw.contains("\"offline.html\""));
        assert!(sw.contains("\"sources/blog/\""));
        // Newest two of three: posts 2 and 1, not 0.
        assert!(sw.contains("\"items/blog/2026-09-03-post-2/\""));
        assert!(sw.contains("\"items/blog/2026-09-02-post-1/\""));
        assert!(!sw.contains("post-0/\""));
        assert_eq!(sw.matches("\"items/").count(), 2);

        let offline = std::fs::read_to_string(out.join("offline.html")).unwrap();
        assert!(offline.contains("data-shell=\"offline\""));
        assert!(offline.contains("<link rel=\"manifest\" href=\"manifest.webmanifest\">"));
        assert!(offline.contains("pwa: true"));
        let river = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(river.contains("rel=\"apple-touch-icon\""));
        assert!(river.contains("name=\"theme-color\""));
        assert!(river.contains("href=\"sources/\""), "{river}");
        assert!(river.contains("href=\"settings/\""), "{river}");
        assert!(river.contains("aggr.toml ↗</a>"), "{river}");
        assert!(out.join("settings/index.html").is_file());
        assert!(out.join("pagefind/pagefind.js").is_file());
    }

    #[test]
    fn pwa_off_writes_no_manifest_worker_or_offline_page() {
        let dir = tempfile::tempdir().unwrap();
        let (config, sources, store) = fixture(dir.path(), 1, "pwa = false\n");
        let out = dir.path().join("out");
        let summary = build(&config, &sources, &store, dir.path(), &info(out.clone())).unwrap();
        assert_eq!(summary.pages, 1 + 1 + 6);
        for name in ["manifest.webmanifest", "sw.js", "offline.html"] {
            assert!(!out.join(name).exists(), "{name} was written");
        }
        let river = std::fs::read_to_string(out.join("index.html")).unwrap();
        assert!(!river.contains("rel=\"manifest\""));
        assert!(river.contains("pwa: false"));
    }
}
