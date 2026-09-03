//! minijinja environment with the layered template/static lookup: project overrides → theme →
//! embedded default.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use minijinja::{Environment, Error, ErrorKind, Value};
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
        let mut names: Vec<String> = DefaultTheme::iter()
            .filter_map(|path| path.strip_prefix("static/").map(str::to_string))
            .collect();
        for dir in &self.dirs {
            let root = dir.join("static");
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
    pub fn new(layers: Layers, _base_path: &str) -> Result<Self> {
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
        let filter_assets = assets.clone();
        // Paths are ours (slugified ASCII), so `/` must not come out as `&#x2f;`.
        env.add_filter("url_for", move |path: String| {
            let path = path.trim_start_matches('/');
            let resolved = path
                .strip_prefix("assets/")
                .and_then(|name| filter_assets.get(name))
                .map(|asset| format!("assets/{}", asset.output))
                .unwrap_or_else(|| path.to_string());
            Value::from_safe_string(resolved)
        });
        env.add_filter("domain", super::context::domain_of);
        env.add_filter("slug", |value: String| slug::slugify(value));
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
    use super::*;

    #[test]
    fn embedded_theme_has_the_required_templates() {
        for name in [
            "base.html",
            "index.html",
            "item.html",
            "sources.html",
            "settings.html",
            "404.html",
            "offline.html",
            "manifest.webmanifest",
            "sw.js",
        ] {
            assert!(
                DefaultTheme::get(&format!("templates/{name}")).is_some(),
                "missing embedded template {name}"
            );
        }
    }

    #[test]
    fn embedded_theme_only_recolors_the_24_hour_boundary() {
        let file = DefaultTheme::get("static/style.css").unwrap();
        let css = std::str::from_utf8(file.data.as_ref()).unwrap();
        assert!(css.contains(".row:not(.age-h24) + .row.age-h24"));
        assert!(!css.contains("age-boundary.age-h1"));
        assert!(!css.contains("age-boundary.age-h3"));
    }

    #[test]
    fn embedded_theme_keeps_article_links_and_navigation_distinct() {
        let css_file = DefaultTheme::get("static/style.css").unwrap();
        let css = std::str::from_utf8(css_file.data.as_ref()).unwrap();
        assert!(css.contains("--accent: #8ea1ff"));
        assert!(css.contains(".body a { color: var(--accent-strong); font-style: normal"));
        assert!(css.contains(".article-more"));
        assert!(css.contains(".main[data-navigation-focus]:focus-visible { outline: none; }"));

        let script_file = DefaultTheme::get("static/app.js").unwrap();
        let script = std::str::from_utf8(script_file.data.as_ref()).unwrap();
        assert!(script.contains("url.searchParams.set(\"q\", query)"));
        assert!(script.contains("KIND === \"river\" && event.key === \"ArrowRight\""));
        assert!(script.contains("event.key === \"ArrowRight\""));
        assert!(script.contains("event.key === \"ArrowLeft\""));
        assert!(script.contains("event.key === \"ArrowLeft\" ? BASE"));

        let item_file = DefaultTheme::get("templates/item.html").unwrap();
        let item = std::str::from_utf8(item_file.data.as_ref()).unwrap();
        assert!(item.contains("for article in item.recommended_articles"));
        assert!(!item.contains("article-more-label"));
        assert!(!item.contains("class=\"permalinks\""));
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
        let renderer = Renderer::new(layers.clone(), "/").unwrap();
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
    fn filters_work() {
        let renderer = Renderer::new(Layers::default(), "/repo/").unwrap();
        let out = renderer
            .render_str_for_test(
                "{{ 'sources/' | url_for }} {{ 'https://www.a.b/c' | domain }} \
                 {{ '2026-09-02T10:00:00Z' | date }} {{ none | date }} {{ '2026-09-02T10:00:00Z' | date('%Y') }}",
            )
            .unwrap();
        assert_eq!(out, "sources/ a.b 2026-09-02  2026");
    }

    #[test]
    fn html_escaping_keeps_slashes_and_blocks_scripts() {
        let mut renderer = Renderer::new(Layers::default(), "/").unwrap();
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
