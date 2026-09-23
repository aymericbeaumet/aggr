//! minijinja environment with the layered template/static lookup: project overrides → theme →
//! embedded default.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use minijinja::{Environment, Error, ErrorKind, State, Value};
use rust_embed::RustEmbed;
use serde::Serialize;

#[derive(RustEmbed)]
#[folder = "themes/default/"]
struct DefaultTheme;

/// Content hash of the embedded fallback theme. This makes build-cache invalidation exact even
/// while developing theme changes without bumping the package version.
pub fn default_theme_hash() -> String {
    let mut names: Vec<_> = DefaultTheme::iter().map(|name| name.into_owned()).collect();
    names.sort();
    let mut bytes = Vec::new();
    for name in names {
        let Some(file) = DefaultTheme::get(&name) else {
            continue;
        };
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(file.data.as_ref());
        bytes.push(0xff);
    }
    crate::model::sha1_hex(&bytes)
}

/// Directories consulted before the embedded theme, most specific first. Each may contain
/// `templates/` and `static/`.
#[derive(Debug, Clone, Default)]
pub struct Layers {
    pub dirs: Vec<PathBuf>,
}

impl Layers {
    /// Reject anything that could escape a layer directory.
    fn check_name(name: &str) -> Result<(), Error> {
        let bad = name.is_empty()
            || name.starts_with('/')
            || name.contains('\\')
            || name
                .split('/')
                .any(|seg| seg.is_empty() || seg == "." || seg == "..");
        if bad {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("invalid template name {name:?}"),
            ));
        }
        Ok(())
    }

    pub fn read(&self, kind: &str, name: &str) -> Result<Option<Cow<'static, [u8]>>, Error> {
        Self::check_name(name)?;
        for dir in &self.dirs {
            let path = dir.join(kind).join(name);
            match std::fs::read(&path) {
                Ok(bytes) => return Ok(Some(Cow::Owned(bytes))),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(Error::new(
                        ErrorKind::InvalidOperation,
                        format!("reading {}: {err}", path.display()),
                    ));
                }
            }
        }
        Ok(DefaultTheme::get(&format!("{kind}/{name}")).map(|file| file.data))
    }

    /// Every static file name across all layers, deduplicated with the most specific winning.
    pub fn static_names(&self) -> Result<Vec<String>> {
        self.names("static")
    }

    /// Every effective template name, including custom includes and the service worker.
    pub fn template_names(&self) -> Result<Vec<String>> {
        self.names("templates")
    }

    fn names(&self, kind: &str) -> Result<Vec<String>> {
        let prefix = format!("{kind}/");
        let mut names: Vec<String> = DefaultTheme::iter()
            .filter_map(|path| path.strip_prefix(&prefix).map(str::to_string))
            .collect();
        for dir in &self.dirs {
            let root = dir.join(kind);
            if !root.is_dir() {
                continue;
            }
            for entry in walkdir::WalkDir::new(&root) {
                let entry = entry.with_context(|| format!("walking {}", root.display()))?;
                if entry.file_type().is_file() {
                    let rel = entry.path().strip_prefix(&root).expect("under root");
                    names.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        names.sort();
        names.dedup();
        Ok(names)
    }
}

pub struct Renderer {
    env: Environment<'static>,
    layers: Layers,
    assets: BTreeMap<String, Asset>,
}

#[derive(Clone)]
struct Asset {
    source: String,
    output: String,
}

impl Renderer {
    pub fn new(layers: Layers) -> Result<Self> {
        let mut env = Environment::new();
        env.set_trim_blocks(true);
        env.set_lstrip_blocks(true);
        env.set_formatter(html_formatter);
        let loader_layers = layers.clone();
        env.set_loader(move |name| {
            loader_layers
                .read("templates", name)?
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .map(Ok)
                .transpose()
        });

        let assets = asset_map(&layers)?;
        // Paths are ours (slugified ASCII), so `/` must not come out as `&#x2f;`.
        // `url_for` is what the browser resolves: it walks up from the page being rendered, so
        // the output works at `/`, under a nested mount, or from a file, and no `<base>` element is
        // needed. `site_path` is the same location relative to the site root, for data attributes
        // the client resolves itself and for joining onto `site.base_url`.
        let filter_assets = assets.clone();
        env.add_filter("url_for", move |state: &State, path: String| {
            if !is_site_reference(&path) {
                // Absolute or remote: escaped like any other value.
                return Value::from(path);
            }
            let path = site_path(&filter_assets, &path);
            let root = page_root(state);
            Value::from_safe_string(if path.is_empty() {
                root
            } else {
                crate::content::rebase_site_url(&path, &root)
            })
        });
        env.add_filter("srcset_for", |state: &State, srcset: String| {
            Value::from(crate::content::rebase_srcset(&srcset, &page_root(state)))
        });
        let filter_assets = assets.clone();
        env.add_filter("site_path", move |path: String| {
            Value::from_safe_string(site_path(&filter_assets, &path))
        });
        env.add_filter("rebase", |state: &State, html: Value| {
            let html = html.as_str().unwrap_or_default();
            Value::from_safe_string(crate::content::rebase_site_paths(html, &page_root(state)))
        });
        env.add_filter("domain", super::context::domain_of);
        env.add_filter("profile", super::context::profile_label);
        env.add_filter("slug", |value: String| slug::slugify(value));
        env.add_filter("facet_url", |state: &State, value: String, kind: String| {
            format!("{}{}", page_root(state), facet_url(value, kind))
        });
        env.add_filter(
            "facet_page",
            |state: &State, value: String, kind: String| {
                format!("{}{}", page_root(state), facet_page(value, kind))
            },
        );
        env.add_filter("date", date_filter);
        env.add_filter("excerpt", |text: String, max: Option<usize>| {
            crate::content::excerpt(&text, max.unwrap_or(200))
        });
        // Safe inside `<script>`: `<` is escaped so a value can never close the tag.
        env.add_filter("json", |value: Value| {
            let json = serde_json::to_string(&value).unwrap_or_default();
            Value::from_safe_string(json.replace('<', "\\u003c"))
        });

        Ok(Self {
            env,
            layers,
            assets,
        })
    }

    pub fn render<S: Serialize>(&self, template: &str, ctx: S) -> Result<String> {
        let template = self
            .env
            .get_template(template)
            .with_context(|| format!("loading template {template}"))?;
        template
            .render(ctx)
            .with_context(|| format!("rendering template {}", template.name()))
    }

    /// Copy every static file into `<out>/assets/`.
    pub fn write_static(&self, out: &Path) -> Result<Vec<String>> {
        for asset in self.assets.values() {
            let Some(bytes) = self
                .layers
                .read("static", &asset.source)
                .map_err(|err| anyhow::anyhow!("{err}"))?
            else {
                bail!("static file {} vanished during build", asset.source);
            };
            let dest = out.join("assets").join(&asset.output);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(&dest, bytes).with_context(|| format!("writing {}", dest.display()))?;
        }
        Ok(self
            .assets
            .values()
            .map(|asset| asset.output.clone())
            .collect())
    }
}

fn asset_map(layers: &Layers) -> Result<BTreeMap<String, Asset>> {
    layers
        .static_names()?
        .into_iter()
        .map(|name| {
            let bytes = layers
                .read("static", &name)
                .map_err(|err| anyhow::anyhow!("{err}"))?
                .with_context(|| format!("static file {name} vanished during build"))?;
            let hash = crate::model::sha1_hex(&bytes);
            let path = Path::new(&name);
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("asset");
            let file = match path.extension().and_then(|extension| extension.to_str()) {
                Some(extension) => format!("{stem}-{}.{}", &hash[..12], extension),
                None => format!("{stem}-{}", &hash[..12]),
            };
            let hashed = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(|parent| parent.join(&file))
                .unwrap_or_else(|| PathBuf::from(file))
                .to_string_lossy()
                .replace('\\', "/");
            let exposed = name.strip_prefix("assets/").unwrap_or(&name).to_string();
            Ok((
                exposed,
                Asset {
                    source: name,
                    output: hashed
                        .strip_prefix("assets/")
                        .unwrap_or(&hashed)
                        .to_string(),
                },
            ))
        })
        .collect()
}

/// minijinja's HTML escaper also rewrites `/` as `&#x2f;`, which turns every URL on the page
/// into noise. Strings get the five characters that matter; everything else keeps the default.
fn html_formatter(
    out: &mut minijinja::Output,
    state: &minijinja::State,
    value: &Value,
) -> Result<(), Error> {
    use std::fmt::Write as _;
    match value.as_str() {
        Some(text) if state.auto_escape() == minijinja::AutoEscape::Html && !value.is_safe() => {
            for ch in text.chars() {
                match ch {
                    '&' => out.write_str("&amp;")?,
                    '<' => out.write_str("&lt;")?,
                    '>' => out.write_str("&gt;")?,
                    '"' => out.write_str("&quot;")?,
                    '\'' => out.write_str("&#x27;")?,
                    ch => out.write_char(ch)?,
                }
            }
            Ok(())
        }
        _ => minijinja::escape_formatter(out, state, value),
    }
}

/// The prefix that takes a site-relative path from the page being rendered to the site root:
/// `page.root` while a page renders, nothing for fragments rendered without one.
fn page_root(state: &State) -> String {
    state
        .lookup("page")
        .and_then(|page| page.get_attr("root").ok())
        .and_then(|root| root.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Whether `url_for` should treat a value as a site path: anything without a scheme. A leading
/// `/` is tolerated as a site path for older templates; fragments and queries pass through.
fn is_site_reference(value: &str) -> bool {
    !value.split_once(':').is_some_and(|(scheme, _)| {
        scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// A site-relative path with content-hashed asset names substituted.
fn site_path(assets: &BTreeMap<String, Asset>, path: &str) -> String {
    let path = path.trim_start_matches('/');
    let path = path.strip_prefix("./").unwrap_or(path);
    path.strip_prefix("assets/")
        .and_then(|name| assets.get(name))
        .map(|asset| format!("assets/{}", asset.output))
        .unwrap_or_else(|| path.to_string())
}

/// The collection page a facet value already has, relative to the site root. A chip navigates to
/// a page the build wrote rather than to a query the browser has to answer: the same list, with
/// no index to download.
fn facet_page(value: String, kind: String) -> String {
    let directory = match kind.as_str() {
        "source" => "sources",
        "category" => "categories",
        _ => "tags",
    };
    format!("{directory}/{value}/")
}

/// The root feed filtered to one facet value, relative to the site root.
fn facet_url(value: String, kind: String) -> String {
    let quoted = serde_json::to_string(&value).unwrap_or_default();
    let query = format!("{kind}:{quoted}");
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("q", &query)
        .finish();
    format!("?{encoded}")
}

/// `{{ value | date }}` → `2026-09-02`; `{{ value | date("%d %b %Y") }}` for custom formats.
/// `none` renders as nothing; other non-date values pass through unchanged.
fn date_filter(value: Value, format: Option<String>) -> Value {
    let Some(text) = value.as_str() else {
        return Value::from("");
    };
    match chrono::DateTime::parse_from_rfc3339(text) {
        Ok(date) => Value::from(
            date.format(format.as_deref().unwrap_or("%Y-%m-%d"))
                .to_string(),
        ),
        Err(_) => value,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// Templates the generator renders directly: the pages `site/mod.rs` writes plus the service
    /// worker and manifest. Every other file under `templates/` must be reached from one of these
    /// through `extends`/`include`, or nothing renders it.
    const RENDERED_TEMPLATES: &[&str] = &[
        "index.html",
        "item.html",
        "browse.html",
        "preferences.html",
        "404.html",
        "offline.html",
        "manifest.webmanifest",
        "sw.js",
    ];

    /// Classes the stylesheet styles that no template, client source or Rust string spells out
    /// as a whole, because the name is assembled from data at build or run time.
    const GENERATED_CLASSES: &[(&str, &str)] = &[
        (
            "syntax-comment",
            "syntect ClassStyle::SpacedPrefixed { prefix: \"syntax-\" } in content_highlight.rs",
        ),
        ("syntax-constant", "syntect scope class, see syntax-comment"),
        ("syntax-entity", "syntect scope class, see syntax-comment"),
        ("syntax-keyword", "syntect scope class, see syntax-comment"),
        ("syntax-name", "syntect scope class, see syntax-comment"),
        ("syntax-storage", "syntect scope class, see syntax-comment"),
        ("syntax-string", "syntect scope class, see syntax-comment"),
    ];

    fn repository_path(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    /// Every UTF-8 file under `root` as (path relative to `root`, contents).
    fn read_tree(root: &Path) -> Vec<(String, String)> {
        walkdir::WalkDir::new(root)
            .sort_by_file_name()
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter_map(|entry| {
                let text = std::fs::read_to_string(entry.path()).ok()?;
                let name = entry
                    .path()
                    .strip_prefix(root)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                Some((name, text))
            })
            .collect()
    }

    /// Class names in the selector preludes of `css`. At-rule preludes and declaration blocks
    /// are not selectors, and quoted attribute values are not classes.
    fn declared_classes(css: &str) -> BTreeSet<String> {
        let comments = regex::Regex::new(r"(?s)/\*.*?\*/").unwrap();
        let strings = regex::Regex::new(r#""[^"]*"|'[^']*'"#).unwrap();
        let class = regex::Regex::new(r"\.([a-z][a-z0-9_-]*)").unwrap();
        let mut classes = BTreeSet::new();
        let mut prelude = String::new();
        for ch in comments.replace_all(css, "").chars() {
            match ch {
                '{' => {
                    if !prelude.trim_start().starts_with('@') {
                        let selector = strings.replace_all(&prelude, "\"\"");
                        classes.extend(
                            class
                                .captures_iter(&selector)
                                .map(|capture| capture[1].to_string()),
                        );
                    }
                    prelude.clear();
                }
                '}' => prelude.clear(),
                ch => prelude.push(ch),
            }
        }
        classes
    }

    /// `name` occurs in `text` as a whole class token, not as part of a longer one.
    fn mentions_class(text: &str, name: &str) -> bool {
        let boundary = |ch: Option<char>| {
            !ch.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        };
        text.match_indices(name).any(|(index, _)| {
            boundary(text[..index].chars().next_back())
                && boundary(text[index + name.len()..].chars().next())
        })
    }

    #[test]
    fn embedded_theme_has_the_required_templates() {
        for name in RENDERED_TEMPLATES.iter().chain(&["base.html"]) {
            assert!(
                DefaultTheme::get(&format!("templates/{name}")).is_some(),
                "missing embedded template {name}"
            );
        }
    }

    #[test]
    fn every_theme_template_is_rendered_or_included() {
        let root = repository_path("themes/default/templates");
        let templates = read_tree(&root);
        assert!(templates.len() >= RENDERED_TEMPLATES.len());
        let reference =
            regex::Regex::new(r#"\{%-?\s*(?:extends|include|import|from)\s+"([^"]+)""#).unwrap();
        let referenced = templates
            .iter()
            .flat_map(|(_, text)| {
                reference
                    .captures_iter(text)
                    .map(|capture| capture[1].to_string())
            })
            .collect::<BTreeSet<_>>();
        let unreachable = templates
            .iter()
            .map(|(name, _)| name.as_str())
            .filter(|name| !RENDERED_TEMPLATES.contains(name) && !referenced.contains(*name))
            .collect::<Vec<_>>();
        assert!(
            unreachable.is_empty(),
            "templates nothing renders or includes: {unreachable:?}"
        );
        for name in &referenced {
            assert!(
                root.join(name).is_file(),
                "included template {name} is missing"
            );
        }
    }

    #[test]
    fn every_class_the_stylesheet_declares_is_rendered_somewhere() {
        let css =
            std::fs::read_to_string(repository_path("themes/default/static/style.css")).unwrap();
        let declared = declared_classes(&css);
        assert!(declared.contains("row") && declared.contains("table-scroll"));
        assert!(!declared.contains("5rem") && !declared.contains("body p"));
        // This file's own assertions must not vouch for a class, and neither may the stylesheet.
        let corpus = ["themes/default/templates", "themes/default/static", "src"]
            .iter()
            .flat_map(|dir| read_tree(&repository_path(dir)))
            .filter(|(name, _)| name != "site/render.rs" && !name.ends_with(".css"))
            .map(|(_, text)| text)
            .collect::<Vec<_>>();
        let unused = declared
            .iter()
            .filter(|name| {
                !GENERATED_CLASSES
                    .iter()
                    .any(|(generated, _)| generated == name)
            })
            .filter(|name| !corpus.iter().any(|text| mentions_class(text, name)))
            .collect::<Vec<_>>();
        assert!(
            unused.is_empty(),
            "style.css declares classes nothing renders: {unused:?}"
        );
        for (generated, _) in GENERATED_CLASSES {
            assert!(
                declared.contains(*generated),
                "{generated} is allowlisted but no longer styled"
            );
        }
    }

    #[test]
    fn embedded_theme_only_recolors_the_24_hour_boundary() {
        let file = DefaultTheme::get("static/style.css").unwrap();
        let css = std::str::from_utf8(file.data.as_ref()).unwrap();
        assert!(css.contains(".row:not(.age-h24):has(+ .row.age-h24)"));
        assert!(css.contains("--row-divider: color-mix"));
        assert!(!css.contains("age-boundary.age-h1"));
        assert!(!css.contains("age-boundary.age-h3"));
    }

    #[test]
    fn every_stretched_link_is_contained_by_the_card_it_covers() {
        // From disk: the embedded copy is only as fresh as the last compile.
        let css =
            std::fs::read_to_string(repository_path("themes/default/static/style.css")).unwrap();
        let css = css.as_str();
        let class = regex::Regex::new(r"\.([a-z][a-z0-9_-]*)").unwrap();
        // Only the element a rule applies to is positioned, never the ancestors that select it.
        let positioned: BTreeSet<String> = rules(css)
            .filter(|(_, block)| block.contains("position: relative"))
            .flat_map(|(prelude, _)| selectors(&prelude))
            .flat_map(|selector| {
                class
                    .captures_iter(&subject(&selector))
                    .map(|capture| capture[1].to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        // An overlay filling `inset: 0` fills its nearest positioned ancestor. Without one of its
        // own it covers whatever surrounds it — the article body reads as the last card's link,
        // underlining on hover and navigating on click.
        let overlays: Vec<_> = rules(css)
            .filter(|(_, block)| block.contains("position: absolute") && block.contains("inset: 0"))
            // Each selector in a group stands on its own: one of them being contained says
            // nothing about the others.
            .flat_map(|(prelude, _)| selectors(&prelude))
            .filter(|selector| selector.contains("::after"))
            .filter(|selector| {
                !class
                    .captures_iter(selector)
                    .any(|capture| positioned.contains(&capture[1]))
            })
            .collect();
        assert!(
            overlays.is_empty(),
            "stretched links with nothing to contain them: {overlays:?}"
        );
    }

    /// The compound a selector applies to: whatever follows its last top-level combinator.
    fn subject(selector: &str) -> String {
        let mut depth = 0usize;
        let mut start = 0usize;
        for (index, character) in selector.char_indices() {
            match character {
                '(' | '[' => depth += 1,
                ')' | ']' => depth = depth.saturating_sub(1),
                ' ' | '>' | '+' | '~' if depth == 0 => start = index + character.len_utf8(),
                _ => {}
            }
        }
        selector[start..].to_string()
    }

    /// A selector group split into its selectors, ignoring the commas inside `:is()` and friends.
    fn selectors(prelude: &str) -> Vec<String> {
        let mut selectors = vec![String::new()];
        let mut depth = 0usize;
        for character in prelude.chars() {
            match character {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    selectors.push(String::new());
                    continue;
                }
                _ => {}
            }
            let last = selectors.len() - 1;
            selectors[last].push(character);
        }
        selectors
            .into_iter()
            .map(|selector| selector.trim().to_string())
            .filter(|selector| !selector.is_empty())
            .collect()
    }

    /// `(prelude, declarations)` for every rule in `css`, comments removed.
    fn rules(css: &str) -> impl Iterator<Item = (String, String)> {
        let css = regex::Regex::new(r"(?s)/\*.*?\*/")
            .unwrap()
            .replace_all(css, "")
            .into_owned();
        css.split('}')
            .filter_map(|rule| {
                let (prelude, block) = rule.split_once('{')?;
                (!prelude.trim_start().starts_with('@'))
                    .then(|| (prelude.trim().to_string(), block.to_string()))
            })
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn embedded_theme_allows_native_vertical_overscroll() {
        let file = DefaultTheme::get("static/style.css").unwrap();
        let css = std::str::from_utf8(file.data.as_ref()).unwrap();
        assert!(css.contains("overscroll-behavior-y: auto"));
        assert!(!css.contains("overscroll-behavior-y: none"));
    }

    #[test]
    fn release_workflow_preserves_the_stable_workflow_tag() {
        let workflow = include_str!("../../.github/workflows/release.yml");
        assert!(workflow.contains("gh release create"));
        assert!(!workflow.contains("git tag --force"));
        assert!(!workflow.contains("git push --force"));
    }

    #[test]
    fn reusable_workflow_caches_exactly_the_derived_state_namespaces() {
        let workflow: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(include_str!("../../.github/workflows/aggr.yml")).unwrap();
        let steps = workflow["jobs"]["aggr"]["steps"].as_sequence().unwrap();
        let mut expected = crate::cache::ci_cached_paths();
        expected.sort();

        let mut restores = Vec::new();
        let mut saves = Vec::new();
        let mut restored_paths = Vec::new();
        let mut saved_paths = Vec::new();
        let mut build = None;
        for (index, step) in steps.iter().enumerate() {
            let uses = step["uses"].as_str().unwrap_or_default();
            assert!(
                !uses.starts_with("actions/cache@"),
                "use explicit restore/save steps so a failed build saves nothing"
            );
            if step["run"]
                .as_str()
                .is_some_and(|run| run.starts_with("aggr build"))
            {
                build = Some(index);
            }
            let Some(path) = step["with"]["path"].as_str() else {
                continue;
            };
            for private in ["render-v1", "articles-v1"] {
                assert!(
                    !path.contains(private),
                    "{uses} must not persist {private}: {path}"
                );
            }
            if !uses.starts_with("actions/cache/restore@")
                && !uses.starts_with("actions/cache/save@")
            {
                continue;
            }
            let paths = path
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if uses.starts_with("actions/cache/restore@") {
                restores.push(index);
                restored_paths.extend(paths);
            } else {
                saves.push(index);
                saved_paths.extend(paths);
            }
        }
        restored_paths.sort();
        saved_paths.sort();
        assert_eq!(
            restored_paths, expected,
            "restore all derived namespaces exactly once"
        );
        assert_eq!(
            saved_paths, expected,
            "save all derived namespaces exactly once"
        );
        let build = build.unwrap();
        assert!(restores.iter().all(|index| *index < build));
        assert!(saves.iter().all(|index| *index > build));
    }

    #[test]
    fn embedded_theme_is_safe_and_readable_on_installed_phones() {
        let file = DefaultTheme::get("static/style.css").unwrap();
        let css = std::str::from_utf8(file.data.as_ref()).unwrap();
        assert!(css.contains("--tap-target: 44px"));
        assert!(css.contains("min-height: 100dvh"));
        assert!(css.contains("env(safe-area-inset-left)"));
        assert!(css.contains("env(safe-area-inset-right)"));
        assert!(css.contains(".itemhead { position: sticky; top: var(--top-nav-offset);"));
        assert!(css.contains("env(safe-area-inset-bottom)"));
        assert!(css.contains("@media (pointer: coarse)"));
        assert!(css.contains("font-size: 16px"));
        assert!(css.contains(".table-scroll:focus-visible"));
        assert!(css.contains("@media (prefers-contrast: more)"));
        assert!(!css.contains("min-height: 24px"));
    }

    #[test]
    fn embedded_theme_presents_and_runtime_caches_article_images() {
        let css_file = DefaultTheme::get("static/style.css").unwrap();
        let css = std::str::from_utf8(css_file.data.as_ref()).unwrap();
        assert!(css.contains(".body .article-picture"));
        assert!(css.contains("var(--image-placeholder, var(--code))"));
        assert!(css.contains("inline-size: min(100%, var(--image-width, 100%))"));
        assert!(css.contains("aspect-ratio: var(--image-ratio)"));
        // Content-addressed media is safe to serve from the cache without revalidating.
        let worker_file = DefaultTheme::get("templates/sw.js").unwrap();
        let worker = std::str::from_utf8(worker_file.data.as_ref()).unwrap();
        assert!(worker.contains("(images|previews)"));
        assert!(worker.contains("assetResponse(request, ASSETS, ASSET_LIMIT)"));
    }

    #[test]
    fn embedded_worker_serves_versioned_search_resources_from_the_cache() {
        let worker_file = DefaultTheme::get("templates/sw.js").unwrap();
        let worker = std::str::from_utf8(worker_file.data.as_ref()).unwrap();
        // The index lives under an immutable version, so it never needs revalidating.
        assert!(worker.contains("pagefind\\/[0-9a-f]{64}\\/"));
        // Nothing is downloaded ahead of the reader: no archive, no index prefetch.
        assert!(!worker.contains("OFFLINE_CATALOG"));
        assert!(!worker.contains("search-manifest.json"));
    }

    #[test]
    fn embedded_navigation_keeps_config_visible() {
        let css_file = DefaultTheme::get("static/style.css").unwrap();
        let css = std::str::from_utf8(css_file.data.as_ref()).unwrap();
        assert!(css.contains(".nav-primary"));
        assert!(css.contains(".nav-actions"));
        assert!(!css.contains(".nav-spacer, .nav-actions, .nav-menu { display: none; }"));

        let base_file = DefaultTheme::get("templates/base.html").unwrap();
        let base = std::str::from_utf8(base_file.data.as_ref()).unwrap();
        assert!(base.contains("class=\"config-link\""));
        assert!(base.contains(">aggr.toml <span aria-hidden=\"true\">↗</span></a>"));
    }

    #[test]
    fn embedded_theme_exposes_progressive_refresh_and_update_controls() {
        let base_file = DefaultTheme::get("templates/base.html").unwrap();
        let base = std::str::from_utf8(base_file.data.as_ref()).unwrap();
        // The shortcut dialog ships as markup; nothing waits for a component to mount.
        assert!(base.contains("<dialog id=\"shortcut-help\""));
        assert!(!base.contains("data-connection-root"));
        assert!(!base.contains("data-shortcut-help-root"));

        let preferences_file = DefaultTheme::get("templates/preferences.html").unwrap();
        let preferences = std::str::from_utf8(preferences_file.data.as_ref()).unwrap();
        assert!(!preferences.contains("id=\"install-app\""));
        assert!(!preferences.contains("preferences-config"));
        assert!(!preferences.contains(">aggr.toml <span aria-hidden=\"true\">↗</span></a>"));
        assert!(preferences.contains("<noscript>"));
        // Every control is rendered from the typed schema, not built in the browser.
        assert!(preferences.contains("site.preference_schema.groups"));
        assert!(preferences.contains("<select"));
        assert!(preferences.contains("data-preference="));
        assert!(!preferences.contains("data-preferences-root"));

        let base_file = DefaultTheme::get("templates/base.html").unwrap();
        let base = std::str::from_utf8(base_file.data.as_ref()).unwrap();
        assert!(base.contains("<script id=\"aggr-preferences\" type=\"application/json\">"));
        // The rules come from the typed table in src/config/preferences.rs; nothing redeclares them.
        assert!(base.contains("site.preference_schema.bootstrap | json"));
        assert!(base.contains("<script src=\"{{ 'assets/bootstrap.js' | url_for }}\"></script>"));
        let bootstrap = DefaultTheme::get("static/bootstrap.js").unwrap();
        let bootstrap = std::str::from_utf8(bootstrap.data.as_ref()).unwrap();
        assert!(bootstrap.contains("AGGRPreferences"));
        assert!(!bootstrap.contains("attribute: \"textSize\""));
        assert!(!base.contains("aggr:reading-history"));

        let index_file = DefaultTheme::get("templates/index.html").unwrap();
        let index = std::str::from_utf8(index_file.data.as_ref()).unwrap();
        assert!(index.contains("data-feed-pager"));
        assert!(index.contains("data-static-page-size"));
        assert!(index.contains("data-total-items"));
        assert!(index.contains("data-page-status"));
    }

    #[test]
    fn embedded_theme_marks_items_discovered_after_an_update() {
        let css_file = DefaultTheme::get("static/style.css").unwrap();
        let css = std::str::from_utf8(css_file.data.as_ref()).unwrap();
        assert!(css.contains(".row.is-new"));
        assert!(css.contains("prefers-reduced-motion: reduce"));
    }

    #[test]
    fn embedded_theme_promotes_footnotes_to_responsive_margin_notes() {
        let css_file = DefaultTheme::get("static/style.css").unwrap();
        let css = std::str::from_utf8(css_file.data.as_ref()).unwrap();
        assert!(css.contains(".footnote-margin-note"));
        assert!(css.contains(".has-margin-notes > .footnotes"));
    }

    #[test]
    fn embedded_theme_unifies_article_navigation_and_recommendations() {
        let renderer = Renderer::new(Layers::default()).unwrap();
        renderer.env.get_template("item.html").unwrap();

        let css_file = DefaultTheme::get("static/style.css").unwrap();
        let css = std::str::from_utf8(css_file.data.as_ref())
            .unwrap()
            .replace("\r\n", "\n");
        assert!(css.contains("--accent: #8ea1ff"));
        assert!(css.contains(".body a { color: var(--accent-strong); font-style: normal"));
        assert!(css.contains(".article-more"));
        assert!(css.contains(".article-more-heading"));
        assert!(!css.contains(".article-navigation-home"));
        assert!(css.contains("input:focus-visible"));
        assert!(!css.contains(
            "input:focus-visible, select:focus-visible {\n  outline: 3px solid var(--accent-strong)"
        ));

        let base_file = DefaultTheme::get("templates/base.html").unwrap();
        let base = std::str::from_utf8(base_file.data.as_ref()).unwrap();
        assert!(base.contains("'preferences/'"));
        assert!(base.contains("rel=\"service-meta\""));
        assert!(base.contains("rel=\"type\""));
        assert!(base.contains("name=\"aggr:network\""));
        assert!(base.contains("max-image-preview:large"));
        assert!(base.contains(
            "<meta name=\"twitter:card\" content=\"{{ 'summary_large_image' if lead_image else 'summary' }}\">"
        ));
        assert!(base.contains("data-nosnippet"));
        assert!(!base.contains("data-route=\"settings/\""));

        let item_file = DefaultTheme::get("templates/item.html").unwrap();
        let item = std::str::from_utf8(item_file.data.as_ref()).unwrap();
        assert!(item.contains("include \"_metadata.html\""));
        let metadata_file = DefaultTheme::get("templates/_metadata.html").unwrap();
        let metadata = std::str::from_utf8(metadata_file.data.as_ref()).unwrap();
        assert!(metadata.contains("class=\"reading-stats\""));
        assert!(css.contains(".shortcut-help {\n  position: fixed;\n  inset: 0;"));
        assert!(css.contains("margin: auto"));

        let item_file = DefaultTheme::get("templates/item.html").unwrap();
        let item = std::str::from_utf8(item_file.data.as_ref()).unwrap();
        assert!(item.contains("<meta name=\"author\""));
        assert!(item.contains("for article in item.recommended_articles"));
        assert!(!item.contains(">Continue reading</h2>"));
        assert!(item.contains(">Coming next</h2>"));
        assert!(item.contains(">Discover more</h2>"));
        assert!(item.contains("aria-labelledby=\"coming-next-title\""));
        assert!(!item.contains("article-more-neighbor"));
        assert!(item.contains("rel=\"next\""));
        assert!(css.contains(".article-footer { display: grid;"));
        assert!(!item.contains("class=\"article-navigation\""));
        assert!(!item.contains(">feed</a>"));
        assert!(!item.contains("class=\"archive-note\""));
        assert!(!item.contains("Archived snapshot"));
        assert!(!item.contains("article-more-label"));
        assert!(!item.contains("class=\"permalinks\""));
    }

    #[test]
    fn article_lead_and_posters_render_the_localised_alt_text() {
        // Only the media blocks are under test; the rest of the page tolerates a sparse context.
        let mut renderer = Renderer::new(Layers::default()).unwrap();
        renderer
            .env
            .set_undefined_behavior(minijinja::UndefinedBehavior::Chainable);
        let placeholder =
            crate::media::placeholder::from_image(&image::DynamicImage::new_rgb8(4, 4)).unwrap();
        let lead =
            crate::site::context::ArticlePreviewCtx::from_image(&crate::content::LocalImage {
                source: "https://publisher.test/lead.jpg".into(),
                original: "assets/images/lead.jpg".into(),
                alt: Some("Apple Watch <on> a \"wrist\"".into()),
                variants: vec![],
                width: 1280,
                height: 720,
                color: "#123456".into(),
                placeholder: placeholder.clone(),
            });
        let preview = crate::site::context::PreviewCtx {
            url: "assets/images/cover.jpg".into(),
            width: 600,
            height: 600,
            alt: Some("Show cover".into()),
            color: None,
            placeholder,
        };
        let item = |media: Value| {
            minijinja::context! {
                title => "Introducing",
                url => "items/blog/introducing/",
                path => "items/blog/introducing.md",
                link => "https://publisher.test/introducing",
                content => "full",
                body_html => "<p>Body</p>",
                labels => Vec::<String>::new(),
                authors => Vec::<String>::new(),
                article_preview => Value::from_serialize(&lead),
                preview => Value::from_serialize(&preview),
                ..media
            }
        };
        let page = |media: Value| {
            renderer
                .render(
                    "item.html",
                    minijinja::context! {
                        item => item(media),
                        site => minijinja::context! { title => "aggr" },
                        page => minijinja::context! { kind => "item" },
                    },
                )
                .unwrap()
        };

        let article = page(minijinja::context! {});
        assert!(
            article.contains("class=\"article-lead media-frame\""),
            "{article}"
        );
        assert!(
            article
                .contains("alt=\"Apple Watch &lt;on&gt; a &quot;wrist&quot;\" decoding=\"async\""),
            "{article}"
        );

        let video = page(minijinja::context! {
            video => minijinja::context! { provider => "youtube", title => "YouTube", embed_url => "https://www.youtube-nocookie.com/embed/x", requires_parent => false },
        });
        assert!(
            video.contains("class=\"video-preview\"")
                && video.contains("alt=\"Apple Watch &lt;on&gt; a &quot;wrist&quot;\""),
            "{video}"
        );

        let audio = page(minijinja::context! {
            native_media => minijinja::context! { kind => "audio", url => "https://publisher.test/episode.mp3" },
        });
        assert!(
            audio.contains(
                "<img class=\"audio-artwork\" src=\"assets/images/cover.jpg\" alt=\"Show cover\""
            ),
            "{audio}"
        );
    }

    #[test]
    fn embedded_theme_groups_collection_directories_under_browse() {
        let renderer = Renderer::new(Layers::default()).unwrap();
        renderer.env.get_template("browse.html").unwrap();

        let base_file = DefaultTheme::get("templates/base.html").unwrap();
        let base = std::str::from_utf8(base_file.data.as_ref()).unwrap();
        assert!(base.contains("data-route=\"browse/\""));
        assert!(base.contains(">browse</"));
        assert!(!base.contains("data-search-action"));
        assert!(base.contains("aria-label=\"Site navigation\""));
        assert!(!base.contains("id=\"site-menu\""));
    }

    #[test]
    fn embedded_theme_selects_a_row_without_a_background_fill() {
        let css_file = DefaultTheme::get("static/style.css").unwrap();
        let css = std::str::from_utf8(css_file.data.as_ref()).unwrap();
        assert!(css.contains(".row.is-selected .title:hover"));
        assert!(css.contains("text-decoration: none"));
        assert!(css.contains(".row.is-selected::before { opacity: 1; }"));
        assert!(!css.contains(".row.is-selected { background:"));

        let item_file = DefaultTheme::get("templates/_item.html").unwrap();
        let item = std::str::from_utf8(item_file.data.as_ref()).unwrap();
        assert!(item.contains("class=\"row-heading\""));
        assert!(css.contains("html:not([data-density=\"comfortable\"]) .row-heading"));
    }

    #[test]
    fn embedded_theme_only_shows_resolved_discussions_without_emphasis() {
        let css_file = DefaultTheme::get("static/style.css").unwrap();
        let css = std::str::from_utf8(css_file.data.as_ref()).unwrap();
        assert!(!css.contains("discussion.is-found"));

        for name in ["_item.html", "item.html"] {
            let file = DefaultTheme::get(&format!("templates/{name}")).unwrap();
            let template = std::str::from_utf8(file.data.as_ref()).unwrap();
            assert!(!template.contains("is-found"), "{name}");
        }
    }

    #[test]
    fn embedded_theme_uses_jk_for_list_selection_and_article_navigation() {
        let css_file = DefaultTheme::get("static/style.css").unwrap();
        let css = std::str::from_utf8(css_file.data.as_ref()).unwrap();
        assert!(!css.contains("reader-page-next-out"));
        assert!(!css.contains("reader-page-previous-in"));
        assert!(css.contains(".itemhead { position: sticky"));
        assert!(css.contains(".itemhead::before"));
        assert!(css.contains(".nav .brand {"));
        assert!(css.contains("margin-inline-start: 0;"));
        assert!(css.contains("margin-inline: calc(-1 * var(--row-bleed))"));

        let item_file = DefaultTheme::get("templates/_metadata.html").unwrap();
        let item = std::str::from_utf8(item_file.data.as_ref()).unwrap();
        assert!(item.contains("data-discussion=\"{{ discussion.name }}\""));
    }

    #[test]
    fn overlay_wins_over_embedded_and_rejects_escapes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("templates")).unwrap();
        std::fs::write(
            dir.path().join("templates/index.html"),
            "custom {{ site.title }}",
        )
        .unwrap();
        let layers = Layers {
            dirs: vec![dir.path().to_path_buf()],
        };
        let renderer = Renderer::new(layers.clone()).unwrap();
        let out = renderer
            .render(
                "index.html",
                minijinja::context! { site => minijinja::context! { title => "T" } },
            )
            .unwrap();
        assert_eq!(out, "custom T");
        assert!(layers.read("templates", "../Cargo.toml").is_err());
        assert!(layers.read("templates", "/etc/passwd").is_err());
        assert!(layers.read("templates", "missing.html").unwrap().is_none());
    }

    #[test]
    fn facet_links_preserve_nested_site_base_and_literal_values() {
        let base = url::Url::parse("https://example.com/reader/").unwrap();
        for value in ["rust", "C++ & web", "a \"quoted\" \\ label", "日本語 #1"] {
            let href = facet_url(value.into(), "tag".into());
            let url = base.join(&href).unwrap();
            assert_eq!(url.path(), "/reader/");
            let query = url.query_pairs().find(|(key, _)| key == "q").unwrap().1;
            assert_eq!(
                query,
                format!("tag:{}", serde_json::to_string(value).unwrap())
            );
        }
    }

    #[test]
    fn filters_work() {
        let renderer = Renderer::new(Layers::default()).unwrap();
        let out = renderer
            .render_str_for_test(
                "{{ 'sources/' | url_for }} {{ 'https://www.a.b/c' | domain }} \
                 {{ '2026-09-02T10:00:00Z' | date }} {{ none | date }} {{ '2026-09-02T10:00:00Z' | date('%Y') }}",
            )
            .unwrap();
        assert_eq!(out, "sources/ a.b 2026-09-02  2026");
        let out = renderer
            .render_str_for_test(
                "{{ 'sources/' | url_for }} {{ '' | url_for }} {{ 'https://example.com/x?a=1&b=2' | url_for }} \
                 {{ '#top' | url_for }} {{ 'assets/images/a.png 1x, https://cdn.example/b,c.png 2x' | srcset_for }} \
                 {{ 'rust' | facet_url('tag') }} {{ 'hnrss.org' | facet_page('source') }} \
                 {{ '<img src=\"assets/x.png\"><a href=\"#n\">n</a>' | rebase }}",
            )
            .unwrap();
        assert_eq!(
            out,
            "sources/  https://example.com/x?a=1&b=2 #top assets/images/a.png 1x, https://cdn.example/b,c.png 2x ?q=tag%3A%22rust%22 sources/hnrss.org/ <img src=\"assets/x.png\"><a href=\"#n\">n</a>"
        );
        let out = renderer
            .env
            .render_str(
                "{{ 'sources/' | url_for }} {{ '' | url_for }} {{ 'https://example.com/x' | url_for }} \
                 {{ 'assets/images/a.png 1x, https://cdn.example/b.png 2x' | srcset_for }} {{ 'rust' | facet_url('tag') }} \
                 {{ '<img src=\"assets/x.png\"><a href=\"#n\">n</a>' | rebase }}",
                minijinja::context! { page => minijinja::context! { root => "../../" } },
            )
            .unwrap();
        assert_eq!(
            out,
            "../../sources/ ../../ https://example.com/x ../../assets/images/a.png 1x, https://cdn.example/b.png 2x ../../?q=tag%3A%22rust%22 <img src=\"../../assets/x.png\"><a href=\"#n\">n</a>"
        );
    }

    #[test]
    fn html_escaping_keeps_slashes_and_blocks_scripts() {
        let mut renderer = Renderer::new(Layers::default()).unwrap();
        renderer
            .env
            .add_template(
                "t.html",
                "<a href=\"{{ link }}\">{{ title }}</a>{{ n }}{{ title | safe }}",
            )
            .unwrap();
        let out = renderer
            .env
            .get_template("t.html")
            .unwrap()
            .render(minijinja::context! {
                link => "https://a.b/c?d=1&e=\"2\"",
                title => "<script>x()</script>",
                n => 3,
            })
            .unwrap();
        assert_eq!(
            out,
            "<a href=\"https://a.b/c?d=1&amp;e=&quot;2&quot;\">&lt;script&gt;x()&lt;/script&gt;</a>3<script>x()</script>"
        );
        let json = renderer
            .env
            .render_str(
                "{% autoescape 'html' %}{{ v | json }}{% endautoescape %}",
                minijinja::context! { v => "</script>" },
            )
            .unwrap();
        assert_eq!(json, "\"\\u003c/script>\"");
    }

    impl Renderer {
        fn render_str_for_test(&self, source: &str) -> Result<String> {
            Ok(self.env.render_str(source, ())?)
        }
    }
}
