//! `aggr.toml`: on-disk schema, defaults, validation, and resolution into engine-ready sources.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use url::Url;

pub const DEFAULT_FILE: &str = "aggr.toml";

/// Every option with its default value, commented. Shipped as `config.default.toml` and written
/// by `aggr init --defaults`.
pub const DEFAULTS: &str = include_str!("../config.default.toml");

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Files (or globs) relative to this one whose `[[sources]]` are appended, in order.
    pub include: Vec<String>,
    pub site: SiteConfig,
    pub store: StoreConfig,
    pub fetch: FetchConfig,
    /// Present → a daily digest is posted as a GitHub issue.
    pub digest: Option<DigestConfig>,
    pub sources: Vec<SourceConfig>,
    /// Root and included TOML files that produced this config; used for exact build-cache keys.
    #[serde(skip)]
    pub(crate) loaded_files: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SiteConfig {
    pub title: String,
    pub description: String,
    pub theme: String,
    pub items_per_page: usize,
    /// Items shown in the recent home feed. Source/category/tag archives remain complete.
    pub max_items: usize,
    pub max_age_days: u32,
    pub max_stubs: usize,
    /// `owner/repo`; defaults to `$GITHUB_REPOSITORY` when unset.
    pub repository: Option<String>,
    /// Public URL of the site (`--release` builds). A custom domain here also writes `CNAME`.
    pub url: Option<Url>,
    pub out: PathBuf,
    /// Emit the web app manifest and service worker (installable, works offline).
    pub pwa: bool,
    /// Newest item pages the service worker caches ahead of time for offline reading.
    pub offline_items: usize,
    /// Links that search for conversations about an item. `{url}` and `{title}` are replaced.
    pub discussions: Vec<DiscussionLinkConfig>,
    /// Free-form values exposed to templates as `site.params`.
    pub params: toml::Table,
}

impl Default for SiteConfig {
    fn default() -> Self {
        Self {
            title: "aggr".into(),
            description: String::new(),
            theme: "default".into(),
            items_per_page: 60,
            max_items: 5000,
            max_age_days: 365,
            max_stubs: 20_000,
            repository: None,
            url: None,
            out: PathBuf::from("_site"),
            pwa: true,
            offline_items: 100,
            discussions: vec![DiscussionLinkConfig {
                name: "Hacker News".into(),
                url: "https://hn.algolia.com/?q={url}".into(),
                provider: Some(DiscussionProvider::HackerNews),
            }],
            params: toml::Table::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiscussionLinkConfig {
    pub name: String,
    pub url: String,
    /// Optional build-time lookup. A failure always falls back to `url`.
    #[serde(default)]
    pub provider: Option<DiscussionProvider>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum DiscussionProvider {
    #[serde(alias = "hn")]
    HackerNews,
    Reddit,
    X,
}

impl DiscussionProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HackerNews => "hackernews",
            Self::Reddit => "reddit",
            Self::X => "x",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StoreConfig {
    pub branch: String,
    /// Worktree of the data branch, relative to the repository root.
    pub dir: PathBuf,
    /// Write the raw `.html` sibling next to each `.md` (per-source override).
    pub html: bool,
    pub html_max_bytes: usize,
    /// Optional tree retention; history stays in git either way.
    pub max_age_days: Option<u32>,
    pub max_items: Option<usize>,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            branch: "aggr".into(),
            dir: PathBuf::from(".aggr/data"),
            html: true,
            html_max_bytes: 262_144,
            max_age_days: None,
            max_items: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FetchConfig {
    pub concurrency: usize,
    /// Concurrent original-article downloads within each source in `heavy` mode.
    pub article_concurrency: usize,
    /// Newest entries considered from one source per sync; avoids an unbounded first import.
    pub max_items_per_source: usize,
    pub timeout_secs: u64,
    /// `{version}` expands to the aggr version.
    pub user_agent: String,
    pub max_body_bytes: usize,
    pub retries: u32,
    /// `heavy` downloads and extracts original article pages; `light` trusts feed content.
    pub content: ContentMode,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            concurrency: 16,
            article_concurrency: 4,
            max_items_per_source: 100,
            timeout_secs: 20,
            user_agent: "aggr/{version} (+https://github.com/aymericbeaumet/aggr)".into(),
            max_body_bytes: 10_000_000,
            retries: 2,
            content: ContentMode::Heavy,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContentMode {
    #[default]
    Heavy,
    Light,
}

impl FetchConfig {
    pub fn user_agent(&self) -> String {
        self.user_agent
            .replace("{version}", env!("CARGO_PKG_VERSION"))
    }
}

/// Daily digest posted as a GitHub issue; GitHub's own notifications deliver it by email.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct DigestConfig {
    /// Local time of day (`HH:MM`) after which the first run posts the digest.
    pub at: chrono::NaiveTime,
    /// IANA timezone used only for the digest schedule.
    pub timezone: chrono_tz::Tz,
    /// Issue title, with `{number}`, `{date}`, `{count}` and `{title}` placeholders.
    pub title: String,
    /// Label put on every digest issue (created on first use).
    pub label: String,
    /// GitHub logins to assign; assigning is what triggers a notification. Defaults to the
    /// repository owner.
    pub assignees: Vec<String>,
    /// Close the previous digest issue when posting a new one.
    pub close_previous: bool,
    /// Skip the day entirely when nothing new arrived.
    pub skip_empty: bool,
    pub max_items: usize,
}

impl Default for DigestConfig {
    fn default() -> Self {
        Self {
            at: chrono::NaiveTime::from_hms_opt(8, 0, 0).expect("valid time"),
            timezone: chrono_tz::UTC,
            title: "Digest #{number} · {date} · {count} new".into(),
            label: "digest".into(),
            assignees: Vec::new(),
            close_previous: true,
            skip_empty: true,
            max_items: 200,
        }
    }
}

/// One `[[sources]]` table as written by the user. Engine-specific keys are validated when the
/// table is resolved, so an unknown key names the offending source.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourceConfig {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub url: Option<String>,
    /// Display name; the upstream feed title is used when unset.
    pub name: Option<String>,
    /// Directory name under `items/`; derived from `name` or `url` when unset.
    pub slug: Option<String>,
    pub category: Option<String>,
    /// Labels applied to every item from this source. Feed-provided labels are appended.
    pub labels: Vec<String>,
    /// Extra request headers; values support `${ENV}` expansion.
    pub headers: BTreeMap<String, String>,
    pub html: Option<bool>,
    /// Override `[fetch] content` for this source.
    pub content: Option<ContentMode>,
    /// `type = "aggr"`: `owner/repo` on GitHub (alternative to a full git `url`).
    pub repo: Option<String>,
    /// `type = "aggr"`: data branch of that repository.
    pub branch: Option<String>,
    /// `type = "aggr"`: only take items from these of its sources (all when empty).
    pub sources: Vec<String>,
    /// `type = "aggr"`: newest items considered per run.
    pub limit: Option<usize>,
}

/// A source after defaults, presets, and `${ENV}` expansion have been applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub slug: String,
    pub name: Option<String>,
    pub category: Option<String>,
    pub labels: Vec<String>,
    pub headers: Vec<(String, String)>,
    pub html: bool,
    pub content: ContentMode,
    pub engine: Engine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Engine {
    Feed {
        url: Url,
    },
    /// Another aggr repository: its data branch is mirrored and its items re-published.
    Aggr {
        /// Git URL; also what humans see as the source URL.
        url: Url,
        branch: String,
        sources: Vec<String>,
        limit: usize,
    },
}

pub const AGGR_SOURCE_LIMIT: usize = 200;

impl Engine {
    pub fn name(&self) -> &'static str {
        match self {
            Engine::Feed { .. } => "web",
            Engine::Aggr { .. } => "aggr",
        }
    }

    /// The URL a human would associate with the source, for status output and `site_url` fallback.
    pub fn url(&self) -> Option<&Url> {
        match self {
            Engine::Feed { url } | Engine::Aggr { url, .. } => Some(url),
        }
    }
}

/// A file named in `include`: sources only, optionally all under one category.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IncludeFile {
    /// Category for every source below that does not set its own.
    pub category: Option<String>,
    pub sources: Vec<SourceConfig>,
}

impl Config {
    /// Parse `path` and append the sources of every file it includes.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut config =
            Self::parse(&text).with_context(|| format!("parsing {}", path.display()))?;
        config.loaded_files.push(
            path.canonicalize()
                .with_context(|| format!("resolving {}", path.display()))?,
        );
        let dir = path.parent().unwrap_or(Path::new("."));
        for pattern in std::mem::take(&mut config.include) {
            for file in include_paths(dir, &pattern)? {
                let text = std::fs::read_to_string(&file)
                    .with_context(|| format!("reading {}", file.display()))?;
                let included: IncludeFile =
                    toml::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
                config
                    .sources
                    .extend(included.sources.into_iter().map(|mut source| {
                        if source.category.is_none() {
                            source.category = included.category.clone();
                        }
                        source
                    }));
                config.loaded_files.push(
                    file.canonicalize()
                        .with_context(|| format!("resolving {}", file.display()))?,
                );
            }
        }
        Ok(config)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.site.items_per_page == 0 {
            bail!("[site] items_per_page must be at least 1");
        }
        if self.fetch.concurrency == 0 {
            bail!("[fetch] concurrency must be at least 1");
        }
        if self.fetch.article_concurrency == 0 {
            bail!("[fetch] article_concurrency must be at least 1");
        }
        if self.fetch.max_items_per_source == 0 {
            bail!("[fetch] max_items_per_source must be at least 1");
        }
        if self.store.branch.is_empty() || self.store.branch.contains(char::is_whitespace) {
            bail!(
                "[store] branch {:?} is not a valid branch name",
                self.store.branch
            );
        }
        if let Some(repo) = &self.site.repository
            && repo.split('/').filter(|part| !part.is_empty()).count() != 2
        {
            bail!("[site] repository must be `owner/repo`, got {repo:?}");
        }
        if let Some(digest) = &self.digest {
            if digest.label.trim().is_empty() {
                bail!("[digest] label must not be empty");
            }
            if digest.max_items == 0 {
                bail!("[digest] max_items must be at least 1");
            }
        }
        Ok(())
    }

    /// Expand every `[[sources]]` table into an engine-ready [`Source`], reading `${ENV}`
    /// references from the process environment.
    pub fn sources(&self) -> Result<Vec<Source>> {
        self.resolve_sources(&|name| std::env::var(name).ok())
    }

    pub fn resolve_sources(&self, env: &dyn Fn(&str) -> Option<String>) -> Result<Vec<Source>> {
        let mut seen = BTreeSet::new();
        let mut sources = Vec::with_capacity(self.sources.len());
        for (index, raw) in self.sources.iter().enumerate() {
            let source = resolve_source(raw, self.fetch.content, env)
                .with_context(|| format!("[[sources]] #{}: {}", index + 1, describe(raw)))?;
            if !seen.insert(source.slug.clone()) {
                bail!(
                    "[[sources]] #{}: slug {:?} is used twice; set `slug` explicitly on one of them",
                    index + 1,
                    source.slug
                );
            }
            sources.push(source);
        }
        Ok(sources)
    }

    /// `owner/repo` for permalinks: explicit config first, then the Actions environment.
    pub fn repository(&self) -> Option<String> {
        self.site
            .repository
            .clone()
            .or_else(|| std::env::var("GITHUB_REPOSITORY").ok())
            .filter(|repo| !repo.is_empty())
    }
}

/// Files an `include` entry names, relative to the config's directory, sorted. A plain path
/// must exist; a glob must match something, so a typo never silently drops a topic.
pub fn include_paths(dir: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let full = dir.join(pattern);
    let is_glob = pattern.contains(['*', '?', '[']);
    if !is_glob {
        if !full.is_file() {
            bail!("include {pattern:?}: {} does not exist", full.display());
        }
        return Ok(vec![full]);
    }
    let pattern_str = full.to_string_lossy();
    let mut paths: Vec<PathBuf> = glob::glob(&pattern_str)
        .with_context(|| format!("include {pattern:?}: invalid glob"))?
        .filter_map(|entry| entry.ok())
        .filter(|path| path.is_file())
        .collect();
    if paths.is_empty() {
        bail!(
            "include {pattern:?} matched no file under {}",
            dir.display()
        );
    }
    paths.sort();
    Ok(paths)
}

fn describe(raw: &SourceConfig) -> String {
    raw.slug
        .clone()
        .or_else(|| raw.name.clone())
        .or_else(|| raw.url.clone())
        .unwrap_or_else(|| "<empty>".into())
}

fn resolve_source(
    raw: &SourceConfig,
    default_content: ContentMode,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Source> {
    let kind = raw.kind.as_deref().unwrap_or("feed");
    let aggr_keys = [
        ("repo", raw.repo.is_some()),
        ("branch", raw.branch.is_some()),
        ("sources", !raw.sources.is_empty()),
        ("limit", raw.limit.is_some()),
    ];
    let only = |owner: &str, keys: &[(&str, bool)]| -> Result<()> {
        for (key, set) in keys {
            if *set {
                bail!("`{key}` only applies to `type = \"{owner}\"` sources");
            }
        }
        Ok(())
    };
    let engine = match kind {
        "feed" => {
            only("aggr", &aggr_keys)?;
            Engine::Feed {
                url: http_url(raw, env)?,
            }
        }
        "aggr" => {
            let url = match (&raw.repo, &raw.url) {
                (Some(repo), None) => {
                    let repo = expand_env(repo, env)?;
                    if repo.split('/').filter(|part| !part.is_empty()).count() != 2
                        || repo.contains("://")
                    {
                        bail!("`repo` must be `owner/repo`, got {repo:?}");
                    }
                    Url::parse(&format!("https://github.com/{}", repo.trim_matches('/')))
                        .context("building the GitHub URL")?
                }
                (None, Some(_)) => http_url(raw, env)?,
                (Some(_), Some(_)) => bail!("set either `repo` or `url`, not both"),
                (None, None) => bail!("`repo` (owner/repo) or `url` (git URL) is required"),
            };
            let limit = raw.limit.unwrap_or(AGGR_SOURCE_LIMIT);
            if limit == 0 {
                bail!("`limit` must be at least 1");
            }
            for slug in &raw.sources {
                validate_slug(slug).context("in `sources`")?;
            }
            Engine::Aggr {
                url,
                branch: raw.branch.clone().unwrap_or_else(|| "aggr".into()),
                sources: raw.sources.clone(),
                limit,
            }
        }
        other => bail!("unknown source type {other:?}; known types: feed, aggr"),
    };

    let slug = match &raw.slug {
        Some(slug) => {
            validate_slug(slug)?;
            slug.clone()
        }
        None => match (&engine, &raw.name) {
            // `owner/repo` reads better than `github-com-owner-repo`.
            (Engine::Aggr { url, .. }, None) => {
                truncate_slug(&slug::slugify(url.path().trim_end_matches(".git")))
            }
            _ => derive_slug(raw.name.as_deref(), engine.url()),
        },
    };

    let headers = raw
        .headers
        .iter()
        .map(|(name, value)| {
            expand_env(value, env)
                .map(|value| (name.clone(), value))
                .with_context(|| format!("header {name:?}"))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Source {
        slug,
        name: raw.name.clone().filter(|name| !name.is_empty()),
        category: raw.category.clone().filter(|category| !category.is_empty()),
        labels: raw.labels.clone(),
        headers,
        html: raw.html.unwrap_or(true),
        content: raw.content.unwrap_or(default_content),
        engine,
    })
}

fn http_url(raw: &SourceConfig, env: &dyn Fn(&str) -> Option<String>) -> Result<Url> {
    let url = raw
        .url
        .as_deref()
        .filter(|url| !url.is_empty())
        .context("`url` is required")?;
    let url = expand_env(url, env)?;
    let url = Url::parse(&url).with_context(|| format!("invalid url {url:?}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("url {url} must use http or https");
    }
    Ok(url)
}

fn validate_slug(slug: &str) -> Result<()> {
    let ok = !slug.is_empty()
        && slug.len() <= 64
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-');
    if !ok {
        bail!("slug {slug:?} must be 1-64 chars of [a-z0-9-], not starting or ending with `-`");
    }
    Ok(())
}

/// Derive a directory-safe slug from the display name, else from the URL's host and path
/// (`https://www.example.com/blog/feed.xml` → `example-com-blog`).
pub fn derive_slug(name: Option<&str>, url: Option<&Url>) -> String {
    if let Some(name) = name {
        let slug = slug::slugify(name);
        if !slug.is_empty() {
            return truncate_slug(&slug);
        }
    }
    let Some(url) = url else {
        return "source".into();
    };
    let host = url
        .host_str()
        .unwrap_or("")
        .trim_start_matches("www.")
        .to_string();
    let path = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|segment| !segment.is_empty() && !is_feed_noise(segment))
        .collect::<Vec<_>>()
        .join("-");
    let slug = slug::slugify(format!("{host} {path}"));
    if slug.is_empty() {
        "source".into()
    } else {
        truncate_slug(&slug)
    }
}

fn is_feed_noise(segment: &str) -> bool {
    let stem = segment
        .rsplit_once('.')
        .filter(|(_, ext)| matches!(*ext, "xml" | "rss" | "atom" | "json" | "php" | "html"))
        .map_or(segment, |(stem, _)| stem);
    matches!(
        stem.to_ascii_lowercase().as_str(),
        "feed" | "feeds" | "rss" | "atom" | "index" | "default" | "posts"
    )
}

fn truncate_slug(slug: &str) -> String {
    if slug.len() <= 64 {
        return slug.to_string();
    }
    let cut = slug[..64].rfind('-').unwrap_or(64);
    slug[..cut].trim_end_matches('-').to_string()
}

/// Replace every `${NAME}` with the variable's value; a missing variable is an error so a
/// misconfigured secret never silently produces an unauthenticated request.
pub fn expand_env(input: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            bail!("unterminated `${{` in {input:?}");
        };
        let name = &after[..end];
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            bail!("invalid environment variable name {name:?} in {input:?}");
        }
        match env(name) {
            Some(value) => out.push_str(&value),
            None => bail!("environment variable {name} is not set"),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn parses_minimal_config_with_defaults() {
        let config =
            Config::parse("[[sources]]\nurl = \"https://example.com/feed.xml\"\n").unwrap();
        assert_eq!(config.site.title, "aggr");
        assert_eq!(config.store.branch, "aggr");
        assert_eq!(config.fetch.concurrency, 16);
        let sources = config.resolve_sources(&no_env).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].slug, "example-com");
        assert_eq!(sources[0].engine.name(), "web");
        assert!(sources[0].html);
    }

    #[test]
    fn rejects_unknown_keys() {
        let err = Config::parse("[site]\ntitel = \"x\"\n").unwrap_err();
        assert!(err.to_string().contains("titel"), "{err}");
        let err = Config::parse("[[sources]]\nurl = \"https://a.b/c\"\nfoo = 1\n").unwrap_err();
        assert!(err.to_string().contains("foo"), "{err}");
    }

    #[test]
    fn rejects_unknown_source_type() {
        let config =
            Config::parse("[[sources]]\ntype = \"telegram\"\nurl = \"https://a.b/\"\n").unwrap();
        let err = config.resolve_sources(&no_env).unwrap_err();
        assert!(
            format!("{err:#}").contains("unknown source type \"telegram\""),
            "{err:#}"
        );
    }

    #[test]
    fn configured_html_engine_is_no_longer_needed() {
        let err =
            Config::parse("[[sources]]\ntype = \"html\"\nurl = \"https://a.b/\"\nitems = \"li\"\n")
                .unwrap_err();
        assert!(err.to_string().contains("items"), "{err:#}");

        let config =
            Config::parse("[[sources]]\ntype = \"html\"\nurl = \"https://a.b/\"\n").unwrap();
        let err = config.resolve_sources(&no_env).unwrap_err();
        assert!(format!("{err:#}").contains("known types: feed, aggr"));
    }

    #[test]
    fn requires_url_for_feed() {
        let config = Config::parse("[[sources]]\nname = \"x\"\n").unwrap();
        let err = config.resolve_sources(&no_env).unwrap_err();
        assert!(format!("{err:#}").contains("`url` is required"), "{err:#}");
    }

    #[test]
    fn rejects_duplicate_slugs() {
        let config = Config::parse(
            "[[sources]]\nurl = \"https://example.com/feed.xml\"\n\
             [[sources]]\nurl = \"https://www.example.com/rss\"\n",
        )
        .unwrap();
        let err = config.resolve_sources(&no_env).unwrap_err();
        assert!(format!("{err:#}").contains("used twice"), "{err:#}");
    }

    #[test]
    fn validates_explicit_slug() {
        let config =
            Config::parse("[[sources]]\nslug = \"Bad Slug\"\nurl = \"https://a.b/\"\n").unwrap();
        assert!(config.resolve_sources(&no_env).is_err());
    }

    #[test]
    fn derives_slugs() {
        let url = |s: &str| Url::parse(s).unwrap();
        assert_eq!(derive_slug(Some("Rust Blog"), None), "rust-blog");
        assert_eq!(
            derive_slug(None, Some(&url("https://blog.rust-lang.org/feed.xml"))),
            "blog-rust-lang-org"
        );
        assert_eq!(
            derive_slug(None, Some(&url("https://www.example.com/blog/feed/"))),
            "example-com-blog"
        );
        assert_eq!(
            derive_slug(
                None,
                Some(&url("https://github.com/rust-lang/rust/releases.atom"))
            ),
            "github-com-rust-lang-rust-releases-atom"
        );
        assert_eq!(
            derive_slug(None, Some(&url("https://hnrss.org/frontpage"))),
            "hnrss-org-frontpage"
        );
        assert_eq!(derive_slug(Some("   "), None), "source");
        let long = "a".repeat(40) + "-" + &"b".repeat(40);
        assert_eq!(derive_slug(Some(&long), None), "a".repeat(40));
    }

    #[test]
    fn expands_env() {
        let env = |name: &str| (name == "TOKEN").then(|| "s3cret".to_string());
        assert_eq!(
            expand_env("Bearer ${TOKEN}", &env).unwrap(),
            "Bearer s3cret"
        );
        assert_eq!(expand_env("plain", &env).unwrap(), "plain");
        assert!(
            expand_env("${MISSING}", &env)
                .unwrap_err()
                .to_string()
                .contains("MISSING")
        );
        assert!(expand_env("${", &env).is_err());
        assert!(expand_env("${bad-name}", &env).is_err());
    }

    #[test]
    fn expands_env_in_url_and_headers() {
        let config = Config::parse(
            "[[sources]]\nurl = \"https://api.example.com/${USER}/feed\"\n\
             headers = { Authorization = \"Bearer ${TOKEN}\" }\n",
        )
        .unwrap();
        let env = |name: &str| match name {
            "USER" => Some("alice".to_string()),
            "TOKEN" => Some("t".to_string()),
            _ => None,
        };
        let sources = config.resolve_sources(&env).unwrap();
        assert_eq!(
            sources[0].engine.url().unwrap().as_str(),
            "https://api.example.com/alice/feed"
        );
        assert_eq!(
            sources[0].headers,
            vec![("Authorization".to_string(), "Bearer t".to_string())]
        );
        let err = config.resolve_sources(&no_env).unwrap_err();
        assert!(format!("{err:#}").contains("USER"), "{err:#}");
    }

    #[test]
    fn shipped_defaults_match_compiled_defaults() {
        let config = Config::parse(DEFAULTS).unwrap();
        let compiled = Config::default();
        assert_eq!(config.site.title, compiled.site.title);
        assert_eq!(config.site.theme, compiled.site.theme);
        assert_eq!(config.site.items_per_page, compiled.site.items_per_page);
        assert_eq!(config.site.max_items, compiled.site.max_items);
        assert_eq!(config.site.max_age_days, compiled.site.max_age_days);
        assert_eq!(config.site.max_stubs, compiled.site.max_stubs);
        assert_eq!(config.site.out, compiled.site.out);
        assert_eq!(config.store.branch, compiled.store.branch);
        assert_eq!(config.store.dir, compiled.store.dir);
        assert_eq!(config.store.html, compiled.store.html);
        assert_eq!(config.store.html_max_bytes, compiled.store.html_max_bytes);
        assert_eq!(config.fetch.concurrency, compiled.fetch.concurrency);
        assert_eq!(
            config.fetch.article_concurrency,
            compiled.fetch.article_concurrency
        );
        assert_eq!(config.fetch.timeout_secs, compiled.fetch.timeout_secs);
        assert_eq!(config.fetch.user_agent, compiled.fetch.user_agent);
        assert_eq!(config.fetch.max_body_bytes, compiled.fetch.max_body_bytes);
        assert_eq!(config.fetch.retries, compiled.fetch.retries);
        assert_eq!(config.fetch.content, compiled.fetch.content);
        assert_eq!(config.site.discussions, compiled.site.discussions);
        assert_eq!(config.digest, Some(DigestConfig::default()));
        assert!(config.sources.is_empty());
    }

    #[test]
    fn parses_digest_time_and_timezone() {
        let config =
            Config::parse("[digest]\nat = \"07:30\"\ntimezone = \"Europe/Paris\"\n").unwrap();
        assert_eq!(
            config.digest.as_ref().unwrap().timezone,
            chrono_tz::Europe::Paris
        );
        assert_eq!(
            config.digest.as_ref().unwrap().at,
            chrono::NaiveTime::from_hms_opt(7, 30, 0).unwrap()
        );
        assert!(Config::parse("[site]\ntimezone = \"UTC\"\n").is_err());
        assert!(Config::parse("[digest]\ntimezone = \"Mars/Olympus\"\n").is_err());
        assert!(Config::parse("[digest]\nat = \"25:00\"\n").is_err());
        assert!(Config::parse("").unwrap().digest.is_none());
    }

    #[test]
    fn heavy_content_is_default_and_sources_can_choose_light() {
        let config = Config::parse(
            "[[sources]]\nurl = \"https://heavy.example/feed\"\n\
             [[sources]]\nurl = \"https://light.example/feed\"\ncontent = \"light\"\n",
        )
        .unwrap();
        let sources = config.sources().unwrap();
        assert_eq!(sources[0].content, ContentMode::Heavy);
        assert_eq!(sources[1].content, ContentMode::Light);
        assert!(Config::parse("[fetch]\ncontent = \"medium\"\n").is_err());
    }

    #[test]
    fn includes_append_sources_with_a_default_category() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("aggr.toml"),
            "include = [\"aggr-ai.toml\", \"topics/*.toml\"]\n[[sources]]\nurl = \"https://a.b/feed\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("aggr-ai.toml"),
            "category = \"ai\"\n[[sources]]\nurl = \"https://ai.example/feed\"\n\
             [[sources]]\nurl = \"https://ml.example/feed\"\ncategory = \"ml\"\n",
        )
        .unwrap();
        std::fs::create_dir(root.join("topics")).unwrap();
        std::fs::write(
            root.join("topics/z.toml"),
            "[[sources]]\nurl = \"https://z.example/feed\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("topics/a.toml"),
            "[[sources]]\nurl = \"https://aa.example/feed\"\n",
        )
        .unwrap();

        let config = Config::load(&root.join("aggr.toml")).unwrap();
        let urls: Vec<_> = config
            .sources
            .iter()
            .map(|s| s.url.clone().unwrap())
            .collect();
        assert_eq!(
            urls,
            [
                "https://a.b/feed",
                "https://ai.example/feed",
                "https://ml.example/feed",
                "https://aa.example/feed",
                "https://z.example/feed",
            ],
            "own sources first, then includes in order, globs sorted"
        );
        assert_eq!(config.sources[1].category.as_deref(), Some("ai"));
        assert_eq!(
            config.sources[2].category.as_deref(),
            Some("ml"),
            "explicit wins"
        );
        assert_eq!(config.sources[3].category, None);
        assert!(config.include.is_empty(), "consumed");

        std::fs::write(root.join("aggr.toml"), "include = [\"missing.toml\"]\n").unwrap();
        let err = Config::load(&root.join("aggr.toml")).unwrap_err();
        assert!(format!("{err:#}").contains("does not exist"), "{err:#}");
        std::fs::write(root.join("aggr.toml"), "include = [\"nope/*.toml\"]\n").unwrap();
        let err = Config::load(&root.join("aggr.toml")).unwrap_err();
        assert!(format!("{err:#}").contains("matched no file"), "{err:#}");
        std::fs::write(root.join("aggr.toml"), "include = [\"bad.toml\"]\n").unwrap();
        std::fs::write(root.join("bad.toml"), "[site]\ntitle = \"x\"\n").unwrap();
        let err = Config::load(&root.join("aggr.toml")).unwrap_err();
        assert!(format!("{err:#}").contains("site"), "only sources: {err:#}");
    }

    #[test]
    fn validates_repository_shape() {
        assert!(Config::parse("[site]\nrepository = \"owner\"\n").is_err());
        assert!(Config::parse("[site]\nrepository = \"owner/repo\"\n").is_ok());
    }

    #[test]
    fn resolves_aggr_sources() {
        let config = Config::parse(
            "[[sources]]\ntype = \"aggr\"\nrepo = \"friend/reads\"\ncategory = \"friends\"\n\n\
             [[sources]]\ntype = \"aggr\"\nurl = \"https://git.example.com/x/reads.git\"\nbranch = \"data\"\nsources = [\"hn\"]\nlimit = 5\n",
        )
        .unwrap();
        let sources = config.resolve_sources(&no_env).unwrap();
        assert_eq!(sources[0].slug, "friend-reads");
        assert_eq!(
            sources[0].engine,
            Engine::Aggr {
                url: Url::parse("https://github.com/friend/reads").unwrap(),
                branch: "aggr".into(),
                sources: vec![],
                limit: AGGR_SOURCE_LIMIT,
            }
        );
        assert_eq!(sources[1].slug, "x-reads");
        assert_eq!(
            sources[1].engine,
            Engine::Aggr {
                url: Url::parse("https://git.example.com/x/reads.git").unwrap(),
                branch: "data".into(),
                sources: vec!["hn".into()],
                limit: 5,
            }
        );

        let bad = |toml: &str| {
            Config::parse(toml)
                .unwrap()
                .resolve_sources(&no_env)
                .is_err()
        };
        assert!(bad("[[sources]]\ntype = \"aggr\"\n"));
        assert!(bad("[[sources]]\ntype = \"aggr\"\nrepo = \"friend\"\n"));
        assert!(bad(
            "[[sources]]\ntype = \"aggr\"\nrepo = \"a/b\"\nurl = \"https://x/\"\n"
        ));
        assert!(bad(
            "[[sources]]\ntype = \"aggr\"\nrepo = \"a/b\"\nlimit = 0\n"
        ));
        assert!(bad(
            "[[sources]]\nurl = \"https://x/feed\"\nrepo = \"a/b\"\n"
        ));
    }
}
