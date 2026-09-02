//! minijinja environment with the layered template/static lookup: project overrides → theme →
//! embedded default.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use minijinja::{Environment, Error, ErrorKind, Value};
use rust_embed::RustEmbed;
use serde::Serialize;

#[derive(RustEmbed)]
#[folder = "themes/default/"]
struct DefaultTheme;

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
}

impl Renderer {
    pub fn new(layers: Layers, base_path: &str) -> Result<Self> {
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

        let prefix = base_path.to_string();
        // Paths are ours (slugified ASCII), so `/` must not come out as `&#x2f;`.
        env.add_filter("url_for", move |path: String| {
            Value::from_safe_string(format!("{prefix}{}", path.trim_start_matches('/')))
        });
        env.add_filter("domain", super::context::domain_of);
        env.add_filter("date", date_filter);
        env.add_filter("excerpt", |text: String, max: Option<usize>| {
            crate::content::excerpt(&text, max.unwrap_or(200))
        });
        // Safe inside `<script>`: `<` is escaped so a value can never close the tag.
        env.add_filter("json", |value: Value| {
            let json = serde_json::to_string(&value).unwrap_or_default();
            Value::from_safe_string(json.replace('<', "\\u003c"))
        });

        Ok(Self { env, layers })
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
        let names = self.layers.static_names()?;
        for name in &names {
            let Some(bytes) = self
                .layers
                .read("static", name)
                .map_err(|err| anyhow::anyhow!("{err}"))?
            else {
                bail!("static file {name} vanished during build");
            };
            let dest = out.join("assets").join(name);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(&dest, bytes).with_context(|| format!("writing {}", dest.display()))?;
        }
        Ok(names)
    }
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
            "html.html",
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
        assert_eq!(out, "/repo/sources/ a.b 2026-09-02  2026");
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
