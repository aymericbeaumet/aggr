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
    pub site: SiteConfig,
    pub store: StoreConfig,
    pub fetch: FetchConfig,
    /// Optional networks searched at build time for conversations about each item.
    pub networks: Vec<NetworkConfig>,
    pub sources: Vec<SourceConfig>,
    /// Root and included TOML files that produced this config; used for exact build-cache keys.
    #[serde(skip)]
    pub(crate) loaded_files: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SiteConfig {
    pub title: String,
    /// BCP 47 language tag used by HTML and syndication formats.
    pub language: String,
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
    /// Free-form values exposed to templates as `site.params`.
    pub params: toml::Table,
}

impl Default for SiteConfig {
    fn default() -> Self {
        Self {
            title: "aggr".into(),
            language: "en".into(),
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
            params: toml::Table::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkConfig {
    pub name: String,
    pub url: String,
    /// Optional build-time lookup. A failure always falls back to `url`.
    pub provider: Option<NetworkProvider>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNetworkConfig {
    #[serde(default)]
    provider: Option<NetworkProvider>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

impl<'de> Deserialize<'de> for NetworkConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = RawNetworkConfig::deserialize(deserializer)?;
        if let Some(provider) = raw.provider {
            if raw.name.is_some() || raw.url.is_some() {
                return Err(serde::de::Error::custom(
                    "a built-in network accepts only `provider`; use `name` + `url` for a custom one",
                ));
            }
            return Ok(provider.network());
        }
        match (raw.name, raw.url) {
            (Some(name), Some(url)) if !name.trim().is_empty() && !url.trim().is_empty() => {
                Ok(Self {
                    name,
                    url,
                    provider: None,
                })
            }
            _ => Err(serde::de::Error::custom(
                "set `provider` for a built-in network, or both `name` and `url` for a custom one",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum NetworkProvider {
    #[serde(alias = "hn")]
    HackerNews,
    Reddit,
    X,
}

impl NetworkProvider {
    fn network(self) -> NetworkConfig {
        let (name, url) = match self {
            Self::HackerNews => ("Hacker News", "https://hn.algolia.com/?q={url}"),
            Self::Reddit => ("Reddit", "https://www.reddit.com/search/?q=url%3A{url}"),
            Self::X => ("X", "https://x.com/search?q={url}"),
        };
        NetworkConfig {
            name: name.into(),
            url: url.into(),
            provider: Some(self),
        }
    }

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

/// One `[[sources]]` table as written by the user. Engine-specific keys are validated when the
/// table is resolved, so an unknown key names the offending source.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourceConfig {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Local TOML file or glob expanded at this position. Mutually exclusive with `url`.
    pub include: Option<String>,
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
    /// Hash of the unexpanded fetch inputs. Safe to commit even when a URL references a secret.
    pub identity: String,
    /// Human-facing URL with credentials and sensitive query values removed.
    pub public_url: Option<String>,
    /// False when a resolved endpoint might contain an expanded secret and must stay off git.
    pub persist_endpoint: bool,
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

/// A file named by a `[[sources]].include`: source entries only.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SourceFile {
    sources: Vec<SourceConfig>,
}

impl Config {
    /// Parse `path` and expand local `[[sources]].include` entries in place.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut config =
            Self::parse(&text).with_context(|| format!("parsing {}", path.display()))?;
        let root = path
            .canonicalize()
            .with_context(|| format!("resolving {}", path.display()))?;
        config.loaded_files.push(root.clone());
        let mut stack = vec![root.clone()];
        config.sources = expand_source_files(
            std::mem::take(&mut config.sources),
            &root,
            None,
            &mut config.loaded_files,
            &mut stack,
        )?;
        Ok(config)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.site.language.trim().is_empty() || self.site.language.contains(char::is_whitespace)
        {
            bail!("[site] language must be a BCP 47 tag such as `en` or `fr-FR`");
        }
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
        for (index, source) in self.sources.iter().enumerate() {
            if source.include.is_some() {
                validate_source_file_entry(source)
                    .with_context(|| format!("[[sources]] #{}", index + 1))?;
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

fn expand_source_files(
    sources: Vec<SourceConfig>,
    declaring_file: &Path,
    inherited_category: Option<&str>,
    loaded_files: &mut Vec<PathBuf>,
    stack: &mut Vec<PathBuf>,
) -> Result<Vec<SourceConfig>> {
    let dir = declaring_file.parent().unwrap_or(Path::new("."));
    let mut expanded = Vec::new();
    for (index, mut source) in sources.into_iter().enumerate() {
        let Some(pattern) = source_file_pattern(&source).map(str::to_string) else {
            if source.category.as_deref().is_none_or(str::is_empty) {
                source.category = inherited_category.map(str::to_string);
            }
            expanded.push(source);
            continue;
        };
        validate_source_file_entry(&source).with_context(|| {
            format!("[[sources]] #{} in {}", index + 1, declaring_file.display())
        })?;
        let category = source
            .category
            .as_deref()
            .filter(|category| !category.is_empty())
            .or(inherited_category)
            .map(str::to_string);
        for file in source_file_paths(dir, &pattern)? {
            let canonical = file
                .canonicalize()
                .with_context(|| format!("resolving {}", file.display()))?;
            if let Some(start) = stack.iter().position(|ancestor| ancestor == &canonical) {
                let mut cycle: Vec<_> = stack[start..]
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect();
                cycle.push(canonical.display().to_string());
                bail!("source-file cycle: {}", cycle.join(" -> "));
            }
            let text = std::fs::read_to_string(&canonical)
                .with_context(|| format!("reading {}", canonical.display()))?;
            let included: SourceFile = toml::from_str(&text)
                .with_context(|| format!("parsing {}", canonical.display()))?;
            loaded_files.push(canonical.clone());
            stack.push(canonical.clone());
            let nested = expand_source_files(
                included.sources,
                &canonical,
                category.as_deref(),
                loaded_files,
                stack,
            );
            stack.pop();
            expanded.extend(nested?);
        }
    }
    Ok(expanded)
}

fn source_file_pattern(source: &SourceConfig) -> Option<&str> {
    source.include.as_deref()
}

fn validate_source_file_entry(source: &SourceConfig) -> Result<()> {
    if source.url.is_some() {
        bail!("`url` and `include` are mutually exclusive");
    }
    let has_other_key = source.kind.is_some()
        || source.name.is_some()
        || source.slug.is_some()
        || !source.labels.is_empty()
        || !source.headers.is_empty()
        || source.html.is_some()
        || source.content.is_some()
        || source.repo.is_some()
        || source.branch.is_some()
        || !source.sources.is_empty()
        || source.limit.is_some();
    if has_other_key {
        bail!("an included TOML source may only set `include` and `category`");
    }
    if source.include.as_deref().is_none_or(str::is_empty) {
        bail!("`include` must name a local TOML file or glob");
    }
    Ok(())
}

/// Files an `include` names, relative to the declaring file, sorted. A plain path must exist; a
/// glob must match something, so a typo never silently drops a topic.
fn source_file_paths(dir: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let full = dir.join(pattern);
    let is_glob = pattern.contains(['*', '?', '[']);
    if !is_glob {
        if !full.is_file() {
            bail!("source file {pattern:?}: {} does not exist", full.display());
        }
        return Ok(vec![full]);
    }
    let pattern_str = full.to_string_lossy();
    let mut paths: Vec<PathBuf> = glob::glob(&pattern_str)
        .with_context(|| format!("source file {pattern:?}: invalid glob"))?
        .filter_map(|entry| entry.ok())
        .filter(|path| path.is_file())
        .collect();
    if paths.is_empty() {
        bail!(
            "source file {pattern:?} matched no file under {}",
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
        .or_else(|| raw.include.clone())
        .unwrap_or_else(|| "<empty>".into())
}

fn resolve_source(
    raw: &SourceConfig,
    default_content: ContentMode,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Source> {
    if raw.include.is_some() {
        bail!("`include` entries must be loaded from an aggr.toml file before resolving sources");
    }
    let kind = raw.kind.as_deref().unwrap_or("feed");
    let identity = source_identity(raw, kind);
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

    let effective_url = engine.url();
    let persist_endpoint = !configured_url_is_sensitive(raw.url.as_deref());
    Ok(Source {
        slug,
        name: raw.name.clone().filter(|name| !name.is_empty()),
        category: raw.category.clone().filter(|category| !category.is_empty()),
        labels: raw.labels.clone(),
        identity,
        public_url: effective_url.map(|url| public_url(url, !persist_endpoint)),
        persist_endpoint,
        headers,
        html: raw.html.unwrap_or(true),
        content: raw.content.unwrap_or(default_content),
        engine,
    })
}

fn source_identity(raw: &SourceConfig, kind: &str) -> String {
    let headers = raw
        .headers
        .iter()
        .map(|(name, value)| format!("{name}:{value}"))
        .collect::<Vec<_>>()
        .join("\n");
    crate::model::sha1_hex(format!(
        "source-v2\0{kind}\0{}\0{}\0{}\0{}\0{headers}",
        raw.url.as_deref().unwrap_or_default(),
        raw.repo.as_deref().unwrap_or_default(),
        raw.branch.as_deref().unwrap_or_default(),
        raw.sources.join(",")
    ))
}

fn configured_url_is_sensitive(raw: Option<&str>) -> bool {
    let Some(raw) = raw else { return false };
    if raw.contains("${") {
        return true;
    }
    Url::parse(raw).is_ok_and(|url| {
        !url.username().is_empty()
            || url.password().is_some()
            || url
                .query_pairs()
                .any(|(key, _)| sensitive_query_key(key.as_ref()))
    })
}

fn sensitive_query_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    ["token", "key", "secret", "signature", "password", "auth"]
        .iter()
        .any(|needle| key == *needle || key.ends_with(&format!("_{needle}")))
}

/// URL safe for generated pages and committed state. A source containing `${ENV}` has every
/// query value removed because aggr cannot infer which arbitrary parameter carries the secret.
pub fn public_url(url: &Url, strip_query: bool) -> String {
    let mut clean = url.clone();
    let _ = clean.set_username("");
    let _ = clean.set_password(None);
    clean.set_fragment(None);
    if strip_query {
        clean.set_query(None);
    } else {
        let pairs: Vec<_> = clean
            .query_pairs()
            .filter(|(key, _)| !sensitive_query_key(key.as_ref()))
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        clean.set_query(None);
        if !pairs.is_empty() {
            clean.query_pairs_mut().extend_pairs(pairs);
        }
    }
    clean.to_string()
}

fn http_url(raw: &SourceConfig, env: &dyn Fn(&str) -> Option<String>) -> Result<Url> {
    let url = raw
        .url
        .as_deref()
        .filter(|url| !url.is_empty())
        .context("`url` is required")?;
    let url = expand_env(url, env)?;
    let url = Url::parse(&url).map_err(|_| {
        anyhow::anyhow!(
            "url {url:?} must be an absolute http(s) URL; use `include = \"./file.toml\"` for local source files"
        )
    })?;
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
        assert_eq!(sources[0].name, None);
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
    fn expanded_url_secrets_never_enter_source_identity_or_public_url() {
        let config = Config::parse(
            "[[sources]]\nurl = \"https://reader:${TOKEN}@example.com/feed?access=${TOKEN}\"\n",
        )
        .unwrap();
        let sources = config
            .resolve_sources(&|name| (name == "TOKEN").then(|| "sentinel-secret".into()))
            .unwrap();
        let source = &sources[0];
        assert!(!source.identity.contains("sentinel-secret"));
        assert_eq!(
            source.public_url.as_deref(),
            Some("https://example.com/feed")
        );
        assert!(!source.persist_endpoint);
        assert!(
            source
                .engine
                .url()
                .unwrap()
                .as_str()
                .contains("sentinel-secret")
        );
    }

    #[test]
    fn shipped_defaults_match_compiled_defaults() {
        let config = Config::parse(DEFAULTS).unwrap();
        let compiled = Config::default();
        assert_eq!(config.site.title, compiled.site.title);
        assert_eq!(config.site.language, compiled.site.language);
        assert_eq!(config.site.theme, compiled.site.theme);
        assert_eq!(config.site.items_per_page, compiled.site.items_per_page);
        assert_eq!(config.site.max_items, compiled.site.max_items);
        assert_eq!(config.site.max_age_days, compiled.site.max_age_days);
        assert_eq!(config.site.max_stubs, compiled.site.max_stubs);
        assert_eq!(config.site.repository, compiled.site.repository);
        assert_eq!(config.site.url, compiled.site.url);
        assert_eq!(config.site.out, compiled.site.out);
        assert_eq!(config.site.pwa, compiled.site.pwa);
        assert_eq!(config.site.offline_items, compiled.site.offline_items);
        assert_eq!(config.site.params, compiled.site.params);
        assert_eq!(config.store.branch, compiled.store.branch);
        assert_eq!(config.store.dir, compiled.store.dir);
        assert_eq!(config.store.html, compiled.store.html);
        assert_eq!(config.store.html_max_bytes, compiled.store.html_max_bytes);
        assert_eq!(config.store.max_age_days, compiled.store.max_age_days);
        assert_eq!(config.store.max_items, compiled.store.max_items);
        assert_eq!(config.fetch.concurrency, compiled.fetch.concurrency);
        assert_eq!(
            config.fetch.article_concurrency,
            compiled.fetch.article_concurrency
        );
        assert_eq!(
            config.fetch.max_items_per_source,
            compiled.fetch.max_items_per_source
        );
        assert_eq!(config.fetch.timeout_secs, compiled.fetch.timeout_secs);
        assert_eq!(config.fetch.max_body_bytes, compiled.fetch.max_body_bytes);
        assert_eq!(config.fetch.retries, compiled.fetch.retries);
        assert_eq!(config.fetch.content, compiled.fetch.content);
        assert_eq!(config.networks, compiled.networks);
        assert!(compiled.networks.is_empty());
        assert!(config.sources.is_empty());
    }

    #[test]
    fn generated_metadata_and_user_agent_are_not_configurable() {
        let description = Config::parse("[site]\ndescription = \"custom\"\n").unwrap_err();
        assert!(
            description.to_string().contains("description"),
            "{description}"
        );

        let user_agent = Config::parse("[fetch]\nuser_agent = \"other\"\n").unwrap_err();
        assert!(
            user_agent.to_string().contains("user_agent"),
            "{user_agent}"
        );
        assert_eq!(
            crate::http::user_agent(),
            format!(
                "aggr/{} (+https://github.com/aymericbeaumet/aggr)",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    #[test]
    fn network_matching_is_opt_in_with_provider_shorthand() {
        let config = Config::parse(
            r#"
[[networks]]
provider = "hackernews"

[[networks]]
provider = "reddit"

[[networks]]
name = "Lobsters"
url = "https://lobste.rs/search?q={url}"
"#,
        )
        .unwrap();
        assert_eq!(config.networks.len(), 3);
        assert_eq!(config.networks[0], NetworkProvider::HackerNews.network());
        assert_eq!(config.networks[1], NetworkProvider::Reddit.network());
        assert_eq!(config.networks[2].name, "Lobsters");
        assert_eq!(config.networks[2].provider, None);

        for invalid in [
            "[[networks]]\nprovider = \"hackernews\"\nname = \"HN\"\n",
            "[[networks]]\nname = \"Lobsters\"\n",
            "[[networks]]\nurl = \"https://example.com/{url}\"\n",
            "[[site.discussions]]\nprovider = \"hackernews\"\n",
        ] {
            assert!(Config::parse(invalid).is_err(), "accepted {invalid:?}");
        }
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
    fn local_toml_sources_expand_in_place_with_category_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("aggr.toml"),
            "[[sources]]\nurl = \"https://a.b/feed\"\n\
             [[sources]]\ninclude = \"./aggr-ai.toml\"\ncategory = \"ai\"\n\
             [[sources]]\ninclude = \"./topics/*.toml\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("aggr-ai.toml"),
            "[[sources]]\nurl = \"https://ai.example/feed\"\n\
             [[sources]]\nurl = \"https://ml.example/feed\"\ncategory = \"ml\"\n\
             [[sources]]\ninclude = \"./nested.toml\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("nested.toml"),
            "[[sources]]\nurl = \"https://nested.example/feed\"\n",
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
                "https://nested.example/feed",
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
        assert_eq!(
            config.sources[3].category.as_deref(),
            Some("ai"),
            "the outer category flows through a nested include"
        );
        assert_eq!(config.sources[4].category, None);
        assert_eq!(config.loaded_files.len(), 5);

        std::fs::write(
            root.join("aggr.toml"),
            "[[sources]]\ninclude = \"./missing.toml\"\n",
        )
        .unwrap();
        let err = Config::load(&root.join("aggr.toml")).unwrap_err();
        assert!(format!("{err:#}").contains("does not exist"), "{err:#}");
        std::fs::write(
            root.join("aggr.toml"),
            "[[sources]]\ninclude = \"./nope/*.toml\"\n",
        )
        .unwrap();
        let err = Config::load(&root.join("aggr.toml")).unwrap_err();
        assert!(format!("{err:#}").contains("matched no file"), "{err:#}");
        std::fs::write(
            root.join("aggr.toml"),
            "[[sources]]\ninclude = \"./bad.toml\"\n",
        )
        .unwrap();
        std::fs::write(root.join("bad.toml"), "[site]\ntitle = \"x\"\n").unwrap();
        let err = Config::load(&root.join("aggr.toml")).unwrap_err();
        assert!(format!("{err:#}").contains("site"), "only sources: {err:#}");

        std::fs::write(
            root.join("aggr.toml"),
            "[[sources]]\ninclude = \"./aggr.toml\"\n",
        )
        .unwrap();
        let err = Config::load(&root.join("aggr.toml")).unwrap_err();
        assert!(format!("{err:#}").contains("source-file cycle"), "{err:#}");

        std::fs::write(
            root.join("aggr.toml"),
            "[[sources]]\ninclude = \"./nested.toml\"\nname = \"not allowed\"\n",
        )
        .unwrap();
        let err = Config::load(&root.join("aggr.toml")).unwrap_err();
        assert!(
            format!("{err:#}").contains("may only set `include` and `category`"),
            "{err:#}"
        );

        std::fs::write(
            root.join("aggr.toml"),
            "[[sources]]\ninclude = \"./nested.toml\"\nurl = \"https://example.com\"\n",
        )
        .unwrap();
        let err = Config::load(&root.join("aggr.toml")).unwrap_err();
        assert!(
            format!("{err:#}").contains("`url` and `include` are mutually exclusive"),
            "{err:#}"
        );
    }

    #[test]
    fn relative_urls_are_never_treated_as_includes() {
        let config = Config::parse("[[sources]]\nurl = \"./aggr-ai.toml\"\n").unwrap();
        let err = config.resolve_sources(&no_env).unwrap_err();
        assert!(
            format!("{err:#}").contains("use `include = \"./file.toml\"`"),
            "{err:#}"
        );
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
