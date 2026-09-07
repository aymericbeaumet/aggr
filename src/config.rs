//! `aggr.toml`: on-disk schema, defaults, validation, and resolution into engine-ready sources.

mod import_formats;
mod import_graph;
mod source_entries;

use source_entries::deserialize_sources;

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
    #[serde(deserialize_with = "deserialize_sources")]
    pub sources: Vec<SourceConfig>,
    /// Local source documents that produced this config; used for build-cache keys and watching.
    #[serde(skip)]
    pub(crate) loaded_files: Vec<PathBuf>,
    /// Remote config identities and content digests that produced this config.
    #[serde(skip)]
    pub(crate) loaded_remote: Vec<(String, String)>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SiteConfig {
    pub title: String,
    /// Optional site summary used by HTML metadata, feeds, manifests, and discovery outputs.
    pub description: Option<String>,
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
    /// Emit install metadata and cache a bounded offline set in a secure context.
    pub pwa: bool,
    /// Newest item pages the service worker caches ahead of time for offline reading.
    pub offline_items: usize,
    /// Optional public identity attached to the site's Schema.org metadata.
    pub identity: Option<SiteIdentityConfig>,
    /// Free-form values exposed to templates as `site.params`.
    pub params: toml::Table,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteIdentityConfig {
    #[serde(rename = "type")]
    pub kind: SiteIdentityKind,
    pub name: String,
    pub url: Option<Url>,
    #[serde(default)]
    pub same_as: Vec<Url>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SiteIdentityKind {
    Person,
    Organization,
}

impl Default for SiteConfig {
    fn default() -> Self {
        Self {
            title: "aggr".into(),
            description: None,
            language: "en".into(),
            theme: "default".into(),
            items_per_page: 50,
            max_items: 5000,
            max_age_days: 365,
            max_stubs: 20_000,
            repository: None,
            url: None,
            out: PathBuf::from("_site"),
            pwa: true,
            offline_items: 30,
            identity: None,
            params: toml::Table::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkConfig {
    pub name: String,
    pub url: String,
    /// Optional build-time lookup. Item contexts expose only exact matches.
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
    /// Newest entries considered from one feed per sync; avoids an unbounded first import.
    pub max_items_per_source: usize,
    pub timeout_secs: u64,
    pub max_body_bytes: usize,
    pub retries: u32,
    /// Allow a remote collection to expand collections outside its own origin or repository.
    #[serde(alias = "allow_remote_include_chains")]
    pub allow_remote_source_chains: bool,
    /// `heavy` downloads and extracts original article pages; `light` trusts feed content.
    pub content: ContentMode,
    /// Download a small local preview; explicit refresh fills missing previews on old items.
    pub previews: bool,
    /// Archive safe article-body raster images and derive lossless responsive renditions.
    pub images: bool,
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
            allow_remote_source_chains: false,
            content: ContentMode::Heavy,
            previews: true,
            images: true,
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

/// One normalized source candidate, expanded from a `[[sources]]` group.
/// Engine-specific options are validated when it resolves to a Source.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourceConfig {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub url: Option<String>,
    /// Local feed documents are produced only by imports, never by deserializing a URL.
    #[serde(skip)]
    pub(crate) local_feed: Option<PathBuf>,
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
    /// Override `[fetch] previews` for new items from this source.
    pub previews: Option<bool>,
    /// Override `[fetch] images` for new items from this source.
    pub images: Option<bool>,
    /// `type = "aggr"`: `owner/repo` on GitHub (alternative to a full git `url`).
    pub repo: Option<String>,
    /// `type = "aggr"`: data branch of that repository.
    pub branch: Option<String>,
    /// `type = "aggr"`: only take items from these of its sources (all when empty).
    pub sources: Vec<String>,
    /// `type = "aggr"`: optional newest-item limit; omitted imports every retained item.
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
    pub previews: bool,
    pub images: bool,
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
        /// None imports every item retained by the upstream aggr.
        limit: Option<usize>,
    },
}

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

impl Config {
    pub(crate) fn parse_source_document(bytes: &[u8], url: &Url) -> Result<Vec<SourceConfig>> {
        import_formats::parse_bytes(bytes, url).map(|document| document.sources)
    }

    /// Parse `path` and resolve source documents into individual sources in place.
    pub async fn load(path: &Path) -> Result<Self> {
        Self::load_with_github_api(path, None).await
    }

    pub async fn load_offline(path: &Path) -> Result<Self> {
        Self::load_documents(path, None, false).await
    }

    async fn load_with_github_api(path: &Path, github_api: Option<Url>) -> Result<Self> {
        Self::load_documents(path, github_api, true).await
    }

    async fn load_documents(
        path: &Path,
        github_api: Option<Url>,
        load_remote: bool,
    ) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut config =
            Self::parse(&text).with_context(|| format!("parsing {}", path.display()))?;
        let root = path
            .canonicalize()
            .with_context(|| format!("resolving {}", path.display()))?;
        let expansion = import_graph::expand(
            std::mem::take(&mut config.sources),
            root,
            &config.fetch,
            github_api,
            load_remote,
        )
        .await?;
        config.sources = expansion.sources;
        config.loaded_files = expansion.local;
        config.loaded_remote = expansion.remote;
        Ok(config)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self
            .site
            .description
            .as_deref()
            .is_some_and(|description| description.trim().is_empty())
        {
            bail!("[site] description must not be empty");
        }
        if self
            .site
            .identity
            .as_ref()
            .is_some_and(|identity| identity.name.trim().is_empty())
        {
            bail!("[site.identity] name must not be empty");
        }
        if !is_bcp47_language_tag(&self.site.language) {
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
        if self.fetch.max_body_bytes == 0 {
            bail!("[fetch] max_body_bytes must be at least 1");
        }
        if self.fetch.max_body_bytes > crate::cache::MAX_ARTICLE_BODY_BYTES {
            bail!(
                "[fetch] max_body_bytes must not exceed {}",
                crate::cache::MAX_ARTICLE_BODY_BYTES
            );
        }
        if self.store.html_max_bytes > crate::store::MAX_STORED_HTML_BYTES {
            bail!(
                "[store] html_max_bytes must not exceed {}",
                crate::store::MAX_STORED_HTML_BYTES
            );
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
        Ok(())
    }

    /// Expand every `[[sources]]` table into an engine-ready [`Source`], reading `${ENV}`
    /// references from the process environment.
    pub fn sources(&self) -> Result<Vec<Source>> {
        self.resolve_sources(&|name| std::env::var(name).ok())
    }

    pub fn resolve_sources(&self, env: &dyn Fn(&str) -> Option<String>) -> Result<Vec<Source>> {
        let mut seen_slugs = BTreeSet::new();
        let mut seen_identities = BTreeMap::<String, (usize, String)>::new();
        let mut sources = Vec::with_capacity(self.sources.len());
        for (index, raw) in self.sources.iter().enumerate() {
            let source = resolve_source(
                raw,
                self.fetch.content,
                self.fetch.previews,
                self.fetch.images,
                env,
            )
            .with_context(|| format!("[[sources]] #{}: {}", index + 1, describe(raw)))?;
            if let Some((first, slug)) = seen_identities.get(&source.identity) {
                log::warn!(
                    "ignoring duplicate source [[sources]] #{} ({:?}); first declared as #{} ({:?})",
                    index + 1,
                    source.slug,
                    first + 1,
                    slug
                );
                continue;
            }
            if !seen_slugs.insert(source.slug.clone()) {
                bail!(
                    "[[sources]] #{}: slug {:?} is used twice; set `slug` explicitly on one of them",
                    index + 1,
                    source.slug
                );
            }
            seen_identities.insert(source.identity.clone(), (index, source.slug.clone()));
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

fn is_bcp47_language_tag(tag: &str) -> bool {
    const GRANDFATHERED: &[&str] = &[
        "art-lojban",
        "cel-gaulish",
        "en-gb-oed",
        "i-ami",
        "i-bnn",
        "i-default",
        "i-enochian",
        "i-hak",
        "i-klingon",
        "i-lux",
        "i-mingo",
        "i-navajo",
        "i-pwn",
        "i-tao",
        "i-tay",
        "i-tsu",
        "no-bok",
        "no-nyn",
        "sgn-be-fr",
        "sgn-be-nl",
        "sgn-ch-de",
        "zh-guoyu",
        "zh-hakka",
        "zh-min",
        "zh-min-nan",
        "zh-xiang",
    ];

    if !tag.is_ascii() {
        return false;
    }
    if GRANDFATHERED
        .iter()
        .any(|known| tag.eq_ignore_ascii_case(known))
    {
        return true;
    }

    let subtags = tag.split('-').collect::<Vec<_>>();
    if subtags.iter().any(|part| {
        part.is_empty() || part.len() > 8 || !part.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }) {
        return false;
    }
    let Some(language) = subtags.first().copied() else {
        return false;
    };
    if language.eq_ignore_ascii_case("x") {
        return subtags.len() > 1;
    }
    if !(2..=8).contains(&language.len())
        || !language.bytes().all(|byte| byte.is_ascii_alphabetic())
    {
        return false;
    }

    let mut index = 1;
    if language.len() <= 3 {
        for _ in 0..3 {
            if subtags.get(index).is_some_and(|part| {
                part.len() == 3 && part.bytes().all(|byte| byte.is_ascii_alphabetic())
            }) {
                index += 1;
            } else {
                break;
            }
        }
    }
    if subtags
        .get(index)
        .is_some_and(|part| part.len() == 4 && part.bytes().all(|byte| byte.is_ascii_alphabetic()))
    {
        index += 1;
    }
    if subtags.get(index).is_some_and(|part| {
        (part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_alphabetic()))
            || (part.len() == 3 && part.bytes().all(|byte| byte.is_ascii_digit()))
    }) {
        index += 1;
    }

    let mut variants = BTreeSet::new();
    while let Some(part) = subtags.get(index).copied()
        && ((5..=8).contains(&part.len())
            || (part.len() == 4 && part.as_bytes()[0].is_ascii_digit()))
    {
        if !variants.insert(part.to_ascii_lowercase()) {
            return false;
        }
        index += 1;
    }

    let mut singletons = BTreeSet::new();
    while let Some(singleton) = subtags
        .get(index)
        .copied()
        .filter(|part| part.len() == 1 && !part.eq_ignore_ascii_case("x"))
    {
        if !singletons.insert(singleton.to_ascii_lowercase()) {
            return false;
        }
        index += 1;
        let start = index;
        while subtags
            .get(index)
            .is_some_and(|part| (2..=8).contains(&part.len()))
        {
            index += 1;
        }
        if index == start {
            return false;
        }
    }

    if subtags
        .get(index)
        .is_some_and(|part| part.eq_ignore_ascii_case("x"))
    {
        index += 1;
        if index == subtags.len() {
            return false;
        }
        index = subtags.len();
    }
    index == subtags.len()
}

fn describe(raw: &SourceConfig) -> String {
    raw.slug
        .clone()
        .or_else(|| raw.name.clone())
        .or_else(|| raw.url.clone())
        .unwrap_or_else(|| "<empty>".into())
}

fn validate_source_options(raw: &SourceConfig) -> Result<()> {
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
    match raw.kind.as_deref().unwrap_or("feed") {
        "feed" => only("aggr", &aggr_keys)?,
        "aggr" => {
            if raw.limit == Some(0) {
                bail!("`limit` must be at least 1; omit it to import all retained items");
            }
            for slug in &raw.sources {
                validate_slug(slug).context("in `sources`")?;
            }
        }
        other => bail!("unknown source type {other:?}; known types: feed, aggr"),
    }
    if let Some(slug) = &raw.slug {
        validate_slug(slug)?;
    }
    Ok(())
}

fn resolve_source(
    raw: &SourceConfig,
    default_content: ContentMode,
    default_previews: bool,
    default_images: bool,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Source> {
    let kind = raw.kind.as_deref().unwrap_or("feed");
    let identity = source_identity(raw, kind);
    validate_source_options(raw)?;
    let engine = match kind {
        "feed" => Engine::Feed {
            url: match &raw.local_feed {
                Some(path) => Url::from_file_path(path)
                    .map_err(|_| anyhow::anyhow!("invalid local feed path"))?,
                None => http_url(raw, env)?,
            },
        },
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
            Engine::Aggr {
                url,
                branch: raw.branch.clone().unwrap_or_else(|| "aggr".into()),
                sources: raw.sources.clone(),
                limit: raw.limit,
            }
        }
        other => bail!("unknown source type {other:?}; known types: feed, aggr"),
    };

    let slug = match &raw.slug {
        Some(slug) => {
            validate_slug(slug)?;
            slug.clone()
        }
        None if raw.local_feed.is_some() => raw
            .name
            .as_deref()
            .map(slug::slugify)
            .filter(|slug| !slug.is_empty())
            .or_else(|| {
                raw.local_feed
                    .as_ref()
                    .and_then(|path| path.file_stem())
                    .and_then(|stem| stem.to_str())
                    .map(slug::slugify)
                    .filter(|slug| !slug.is_empty())
            })
            .map(|slug| truncate_slug(&slug))
            .unwrap_or_else(|| "source".into()),
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
    let persist_endpoint =
        raw.local_feed.is_none() && !configured_url_is_sensitive(raw.url.as_deref());
    Ok(Source {
        slug,
        name: raw.name.clone().filter(|name| !name.is_empty()),
        category: raw
            .category
            .as_deref()
            .and_then(crate::model::normalize_category),
        labels: crate::model::normalize_labels(&raw.labels),
        identity,
        public_url: effective_url
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .map(|url| public_url(url, !persist_endpoint)),
        persist_endpoint,
        headers,
        html: raw.html.unwrap_or(true),
        content: raw.content.unwrap_or(default_content),
        previews: raw.previews.unwrap_or(default_previews),
        images: raw.images.unwrap_or(default_images),
        engine,
    })
}

fn source_identity(raw: &SourceConfig, kind: &str) -> String {
    if let Some(path) = &raw.local_feed {
        return crate::model::sha1_hex(format!("local-feed\0{}", path.display()));
    }
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
    if url.contains(char::is_whitespace) {
        bail!("each `url` line must contain one URL without whitespace");
    }
    let url = Url::parse(&url).map_err(|_| {
        anyhow::anyhow!(
            "url {url:?} must be an absolute http(s) URL; load the config from a file to resolve local source documents"
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
    fn source_taxonomies_are_normalized_without_changing_source_names() {
        let config = Config::parse(
            r##"[[sources]]
url = "https://example.com/feed"
name = "The Example Blog"
category = "  Computer   SCIENCE  "
labels = ["#Rust", "RUST", "Generative AI"]
"##,
        )
        .unwrap();
        let sources = config.resolve_sources(&no_env).unwrap();
        assert_eq!(sources[0].name.as_deref(), Some("The Example Blog"));
        assert_eq!(sources[0].category.as_deref(), Some("computer science"));
        assert_eq!(sources[0].labels, ["generative ai", "rust"]);
    }

    #[test]
    fn local_feed_slug_fallback_never_contains_parent_directories() {
        for name in [None, Some(""), Some("   "), Some("!!!")] {
            let config = Config {
                sources: vec![SourceConfig {
                    local_feed: Some(PathBuf::from("/private/home/reader/feeds/news.xml")),
                    name: name.map(str::to_owned),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let sources = config.sources().unwrap();
            assert_eq!(sources[0].slug, "news");
            assert_eq!(sources[0].public_url, None);
            assert!(!sources[0].persist_endpoint);
        }
    }

    #[test]
    fn source_forms_share_normalization_and_resolution() {
        let expected = Config::parse("[[sources]]\nurl='https://one.example/feed#part'\ncategory='AI'\nlabels=['research']\n[[sources]]\nurl='https://two.example/'\ncategory='AI'\nlabels=['research']")
            .unwrap().resolve_sources(&no_env).unwrap();
        {
            let key = "url";
            for value in [
                r#""  https://one.example/feed#part\r\n\n https://two.example/\t ""#,
                r#"[" https://one.example/feed#part ", "", "https://two.example/"]"#,
                r#"[" \nhttps://one.example/feed#part\n https://two.example/\n", " "]"#,
                "'''\n  https://one.example/feed#part\n\n  https://two.example/\n'''",
            ] {
                let text =
                    format!("[[sources]]\n{key}={value}\ncategory='AI'\nlabels=['research']");
                let actual = Config::parse(&text)
                    .unwrap()
                    .resolve_sources(&no_env)
                    .unwrap();
                assert_eq!(actual, expected, "{text}");
            }
        }
    }

    #[test]
    fn source_blocks_can_mix_forms_between_sections_in_declaration_order() {
        let config = Config::parse(
            r#"
[site]
title = "Mixed"
[[sources]]
url = "https://one.example/"
category = "first"
[fetch]
content = "light"
[[sources]]
url = ["https://two.example/", "https://three.example/"]
category = "second"
[[networks]]
provider = "reddit"
[[sources]]
url = "https://one.example/"
name = "Duplicate"
[[sources]]
url = "https://${HOST}/"
headers = { Authorization = "Bearer ${TOKEN}" }
images = false
"#,
        )
        .unwrap();
        let sources = config
            .resolve_sources(&|key| match key {
                "HOST" => Some("four.example".into()),
                "TOKEN" => Some("secret".into()),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            sources.iter().map(|s| s.slug.as_str()).collect::<Vec<_>>(),
            [
                "one-example",
                "two-example",
                "three-example",
                "four-example"
            ]
        );
        assert_eq!(sources[0].category.as_deref(), Some("first"));
        assert_eq!(sources[1].category.as_deref(), Some("second"));
        assert_eq!(sources[2].category.as_deref(), Some("second"));
        assert_eq!(sources[3].category, None);
        assert_eq!(
            sources[3].headers,
            [("Authorization".into(), "Bearer secret".into())]
        );
        assert!(!sources[3].images);
        assert!(sources.iter().all(|s| s.content == ContentMode::Light));
    }

    #[test]
    fn source_selectors_reject_conflicts_types_and_removed_syntax() {
        {
            let key = "url";
            for invalid in ["42", "[42]", "[{url='https://example.com/'}]"] {
                assert!(Config::parse(&format!("[[sources]]\n{key}={invalid}")).is_err());
            }
            for empty in ["''", "[]", r#"["", " \n\t"]"#] {
                assert!(
                    Config::parse(&format!("[[sources]]\n{key}={empty}"))
                        .unwrap()
                        .sources
                        .is_empty()
                );
                assert!(Config::parse(&format!("[[sources]]\n{key}={empty}\ntypo=true")).is_err());
            }
        }
        {
            let key = "url";
            for invalid in [
                r##"#[category="ai"]"##,
                r##"https://one.example/ #[category="ai"]"##,
                "https://one.example/ https://two.example/",
                "file:///tmp/feed.xml",
                "ftp://example.com/feed",
            ] {
                assert!(
                    Config::parse(&format!("[[sources]]\n{key}={invalid:?}"))
                        .and_then(|c| c.resolve_sources(&no_env))
                        .is_err(),
                    "{key}: {invalid}"
                );
            }
        }
        assert!(Config::parse("sources_urls=[]").is_err());
        for options in [
            "type='typo'",
            "slug='INVALID'",
            "repo='a/b'",
            "type='aggr'\nlimit=0",
        ] {
            assert!(Config::parse(&format!("[[sources]]\nurl=[]\n{options}")).is_err());
        }
        assert!(Config::parse("[[sources]]\nimport='./one.toml'").is_err());
        assert!(Config::parse("sources=['https://example.com/']").is_err());
    }

    #[tokio::test]
    async fn legacy_include_and_remote_chain_setting_remain_compatible() {
        crate::http::install_crypto_provider();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aggr.toml");
        std::fs::write(
            &root,
            "[fetch]\nallow_remote_include_chains = true\n[[sources]]\ninclude = './topics.toml'\ncategory = 'Programming'\n",
        ).unwrap();
        std::fs::write(
            dir.path().join("topics.toml"),
            "[[sources]]\ninclude = './nested.toml'\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("nested.toml"),
            "[[sources]]\nurl = 'https://example.org/feed'\n",
        )
        .unwrap();
        let config = Config::load_offline(&root).await.unwrap();
        assert!(config.fetch.allow_remote_source_chains);
        let sources = config.sources().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].category.as_deref(), Some("programming"));
        assert_eq!(
            sources[0].public_url.as_deref(),
            Some("https://example.org/feed")
        );
    }

    #[test]
    fn legacy_aliases_reject_ambiguous_and_unknown_fields() {
        for text in [
            "[[sources]]\nurl = './a.toml'\ninclude = './b.toml'",
            "[fetch]\nallow_remote_source_chains = false\nallow_remote_include_chains = true",
            "[[sources]]\ninclude = './a.toml'\ninculde = './b.toml'",
        ] {
            assert!(Config::parse(text).is_err(), "accepted {text}");
        }
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
        assert_eq!(config.site.description, compiled.site.description);
        assert_eq!(config.site.language, compiled.site.language);
        assert_eq!(config.site.theme, compiled.site.theme);
        assert_eq!(config.site.items_per_page, compiled.site.items_per_page);
        assert_eq!(compiled.site.items_per_page, 50);
        assert_eq!(config.site.max_items, compiled.site.max_items);
        assert_eq!(config.site.max_age_days, compiled.site.max_age_days);
        assert_eq!(config.site.max_stubs, compiled.site.max_stubs);
        assert_eq!(config.site.repository, compiled.site.repository);
        assert_eq!(config.site.url, compiled.site.url);
        assert_eq!(config.site.out, compiled.site.out);
        assert_eq!(config.site.pwa, compiled.site.pwa);
        assert_eq!(config.site.offline_items, compiled.site.offline_items);
        assert_eq!(config.site.identity, compiled.site.identity);
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
        assert_eq!(
            config.fetch.allow_remote_source_chains,
            compiled.fetch.allow_remote_source_chains
        );
        assert_eq!(config.fetch.content, compiled.fetch.content);
        assert_eq!(config.fetch.previews, compiled.fetch.previews);
        assert_eq!(config.fetch.images, compiled.fetch.images);
        assert_eq!(config.networks, compiled.networks);
        assert!(compiled.networks.is_empty());
        assert!(config.sources.is_empty());
    }

    #[test]
    fn stored_html_limit_cannot_exceed_the_reader_cap() {
        let maximum = crate::store::MAX_STORED_HTML_BYTES;
        assert!(Config::parse(&format!("[store]\nhtml_max_bytes = {maximum}\n")).is_ok());
        let error =
            Config::parse(&format!("[store]\nhtml_max_bytes = {}\n", maximum + 1)).unwrap_err();
        assert!(error.to_string().contains("html_max_bytes"), "{error:#}");
    }

    #[test]
    fn fetched_body_limit_matches_the_persistent_cache_cap() {
        let maximum = crate::cache::MAX_ARTICLE_BODY_BYTES;
        assert!(Config::parse(&format!("[fetch]\nmax_body_bytes = {maximum}\n")).is_ok());
        for invalid in [0, maximum + 1] {
            let error =
                Config::parse(&format!("[fetch]\nmax_body_bytes = {invalid}\n")).unwrap_err();
            assert!(error.to_string().contains("max_body_bytes"), "{error:#}");
        }
    }

    #[test]
    fn generated_user_agent_is_not_configurable() {
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
    fn previews_are_enabled_and_sources_can_override_the_default() {
        let config = Config::parse("[[sources]]\nurl = 'https://example.com/feed'\n").unwrap();
        assert!(config.fetch.previews);
        assert!(config.sources().unwrap()[0].previews);
        let config = Config::parse("[fetch]\npreviews = true\n[[sources]]\nurl = 'https://a.example/feed'\n[[sources]]\nurl = 'https://b.example/feed'\npreviews = false\n").unwrap();
        let sources = config.sources().unwrap();
        assert!(sources[0].previews);
        assert!(!sources[1].previews);
        assert!(Config::parse("[[sources]]\nurl = './other.toml'\npreviews = true\n").is_ok());
    }

    #[test]
    fn article_images_are_local_by_default_and_sources_can_opt_out() {
        let config = Config::parse("[[sources]]\nurl = 'https://example.com/feed'\n").unwrap();
        assert!(config.fetch.images);
        assert!(config.sources().unwrap()[0].images);
        let config = Config::parse("[fetch]\nimages = false\n[[sources]]\nurl = 'https://a.example/feed'\n[[sources]]\nurl = 'https://b.example/feed'\nimages = true\n").unwrap();
        let sources = config.sources().unwrap();
        assert!(!sources[0].images);
        assert!(sources[1].images);
        assert!(Config::parse("[[sources]]\nurl = './other.toml'\nimages = true\n").is_ok());
    }

    #[test]
    fn site_description_and_identity_are_configurable() {
        let config = Config::parse(
            r#"
[site]
description = "Independent reading notes and useful links."

[site.identity]
type = "person"
name = "Ada Example"
url = "https://example.com/ada"
same_as = ["https://social.example/@ada"]
"#,
        )
        .unwrap();
        assert_eq!(
            config.site.description.as_deref(),
            Some("Independent reading notes and useful links.")
        );
        let identity = config.site.identity.unwrap();
        assert_eq!(identity.kind, SiteIdentityKind::Person);
        assert_eq!(identity.name, "Ada Example");
        assert_eq!(identity.url.unwrap().as_str(), "https://example.com/ada");
        assert_eq!(identity.same_as[0].as_str(), "https://social.example/@ada");

        let empty_description = Config::parse("[site]\ndescription = \" \"\n").unwrap_err();
        assert!(
            empty_description.to_string().contains("description"),
            "{empty_description}"
        );
        let empty_identity =
            Config::parse("[site.identity]\ntype = \"organization\"\nname = \" \"\n").unwrap_err();
        assert!(
            empty_identity.to_string().contains("identity"),
            "{empty_identity}"
        );
    }

    #[test]
    fn site_language_requires_a_well_formed_bcp_47_tag() {
        for language in [
            "en",
            "fr-FR",
            "zh-Hant-TW",
            "zh-cmn-Hans-CN",
            "de-CH-1901",
            "en-US-u-ca-gregory",
            "x-reader-local",
            "i-klingon",
        ] {
            let config = format!("[site]\nlanguage = {language:?}\n");
            assert!(Config::parse(&config).is_ok(), "rejected {language:?}");
        }

        for language in [
            "",
            "e",
            "en_US",
            "en--US",
            "en-",
            "en-abcdefghi",
            "en-u",
            "en-u-ca-u-nu",
            "de-1901-1901",
            "x",
            "123",
            "fr-É",
        ] {
            let config = format!("[site]\nlanguage = {language:?}\n");
            let error = Config::parse(&config).unwrap_err();
            assert!(
                error.to_string().contains("BCP 47"),
                "{language:?}: {error}"
            );
        }
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

    #[tokio::test]
    async fn local_toml_sources_expand_in_place_with_category_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("aggr.toml"),
            "[[sources]]\nurl = \"https://a.b/feed\"\n\
             [[sources]]\nurl = \"./aggr-ai.toml\"\ncategory = \"ai\"\n\
             [[sources]]\nurl = \"./topics/*.toml\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("aggr-ai.toml"),
            "[[sources]]\nurl = \"https://ai.example/feed\"\n\
             [[sources]]\nurl = \"https://ml.example/feed\"\ncategory = \"ml\"\n\
             [[sources]]\nurl = \"./nested.toml\"\n",
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

        let config = Config::load(&root.join("aggr.toml")).await.unwrap();
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
            "[[sources]]\nurl = \"./missing.toml\"\n",
        )
        .unwrap();
        let config = Config::load(&root.join("aggr.toml")).await.unwrap();
        assert!(config.sources.is_empty(), "missing includes are ignored");
        std::fs::write(
            root.join("aggr.toml"),
            "[[sources]]\nurl = \"./nope/*.toml\"\n",
        )
        .unwrap();
        let config = Config::load(&root.join("aggr.toml")).await.unwrap();
        assert!(config.sources.is_empty(), "empty globs are ignored");
        std::fs::write(
            root.join("aggr.toml"),
            "[[sources]]\nurl = \"./bad.toml\"\n",
        )
        .unwrap();
        std::fs::write(root.join("bad.toml"), "[site]\ntitle = \"x\"\n").unwrap();
        let config = Config::load(&root.join("aggr.toml")).await.unwrap();
        assert!(
            config.sources.is_empty(),
            "configs without sources are ignored"
        );

        std::fs::write(
            root.join("aggr.toml"),
            "[[sources]]\nurl = \"./aggr.toml\"\n",
        )
        .unwrap();
        let config = Config::load(&root.join("aggr.toml")).await.unwrap();
        assert!(
            config.sources.is_empty(),
            "cycles terminate without aborting"
        );

        std::fs::write(
            root.join("aggr.toml"),
            "[[sources]]\nurl = \"./nested.toml\"\nname = \"shared name\"\n",
        )
        .unwrap();
        let config = Config::load(&root.join("aggr.toml")).await.unwrap();
        assert_eq!(config.sources[0].name.as_deref(), Some("shared name"));

        std::fs::write(
            root.join("aggr.toml"),
            "[[sources]]\nurl = \"./nested.toml\"\nurl = \"https://example.com\"\n",
        )
        .unwrap();
        let err = Config::load(&root.join("aggr.toml")).await.unwrap_err();
        assert!(format!("{err:#}").contains("duplicate key"), "{err:#}");
    }

    #[tokio::test]
    async fn github_configs_infer_root_expand_wildcards_and_stop_remote_hops() {
        use httpmock::Method::GET;
        use httpmock::MockServer;

        let server = MockServer::start_async().await;
        let metadata = server
            .mock_async(|when, then| {
                when.method(GET).path("/repos/owner/reading");
                then.status(200).body(r#"{"default_branch":"main"}"#);
            })
            .await;
        let remote = format!(
            r#"
[site]
title = "ignored remote title"

[[sources]]
url = "https://same.example/feed"

[[sources]]
url = "./topics/*.toml"

[[sources]]
url = "{}/forbidden.toml"
"#,
            server.base_url()
        );
        let root_config = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/repos/owner/reading/contents/aggr.toml");
                then.status(200).body(remote.clone());
            })
            .await;
        let tree = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/repos/owner/reading/git/trees/main")
                    .query_param("recursive", "1");
                then.status(200).body(
                    r#"{"truncated":false,"tree":[
                        {"path":"aggr.toml","type":"blob"},
                        {"path":"topics/b.toml","type":"blob"},
                        {"path":"topics/a.toml","type":"blob"},
                        {"path":"topics/subdir","type":"tree"}
                    ]}"#,
                );
            })
            .await;
        let first = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/repos/owner/reading/contents/topics/a.toml")
                    .query_param("ref", "main");
                then.status(200).body(
                    "[[sources]]\nurl = \"https://a.example/feed\"\n\
                     [[sources]]\nurl = \"../aggr.toml\"\n",
                );
            })
            .await;
        let second = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/repos/owner/reading/contents/topics/b.toml")
                    .query_param("ref", "main");
                then.status(200).body(
                    "[[sources]]\nurl = \"https://b.example/feed\"\n\
                     [[sources]]\nurl = \"https://same.example/feed\"\n",
                );
            })
            .await;

        let forbidden = server
            .mock_async(|when, then| {
                when.method(GET).path("/forbidden.toml");
                then.status(200)
                    .body("[[sources]]\nurl='https://forbidden.example/feed'\n");
            })
            .await;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aggr.toml");
        std::fs::write(
            &path,
            "[[sources]]\nurl = \"https://github.com/owner/reading\"\ncategory = \"shared\"\n",
        )
        .unwrap();
        let config = Config::load_with_github_api(
            &path,
            Some(Url::parse(&format!("{}/", server.base_url())).unwrap()),
        )
        .await
        .unwrap();

        let sources = config.sources().unwrap();
        assert_eq!(
            sources
                .iter()
                .map(|source| source.engine.url().unwrap().as_str())
                .collect::<Vec<_>>(),
            [
                "https://same.example/feed",
                "https://a.example/feed",
                "https://b.example/feed",
            ],
            "GitHub globs are sorted, cycles terminate, forbidden hops are skipped, and the first duplicate wins"
        );
        assert!(
            sources
                .iter()
                .all(|source| source.category.as_deref() == Some("shared"))
        );
        assert_eq!(config.loaded_files.len(), 1);
        assert_eq!(config.loaded_remote.len(), 3);
        forbidden.assert_calls_async(1).await;
        metadata.assert_calls_async(1).await;
        root_config.assert_calls_async(1).await;
        tree.assert_calls_async(1).await;
        first.assert_calls_async(1).await;
        second.assert_calls_async(1).await;
    }

    #[tokio::test]
    async fn the_root_can_explicitly_allow_remote_source_chains() {
        use httpmock::Method::GET;
        use httpmock::MockServer;

        let server = MockServer::start_async().await;
        let other = MockServer::start_async().await;
        let second_url = other.url("/second.toml");
        let first_body = format!(
            "[[sources]]\nurl = \"https://one.example/feed\"\n\
             [[sources]]\nurl = \"{second_url}\"\n"
        );
        let first = server
            .mock_async(|when, then| {
                when.method(GET).path("/first.toml");
                then.status(200).body(first_body.clone());
            })
            .await;
        let second = other
            .mock_async(|when, then| {
                when.method(GET).path("/second.toml");
                then.status(200)
                    .body("[[sources]]\nurl = \"https://two.example/feed\"\n");
            })
            .await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aggr.toml");

        std::fs::write(
            &path,
            format!("[[sources]]\nurl = \"{}\"\n", server.url("/first.toml")),
        )
        .unwrap();
        let config = Config::load(&path).await.unwrap();
        assert_eq!(config.sources().unwrap().len(), 1);
        assert_eq!(second.calls_async().await, 1);

        std::fs::write(
            &path,
            format!(
                "[fetch]\nallow_remote_source_chains = true\n\
                 [[sources]]\nurl = \"{}\"\n",
                server.url("/first.toml")
            ),
        )
        .unwrap();
        let config = Config::load(&path).await.unwrap();
        assert_eq!(config.sources().unwrap().len(), 2);
        assert_eq!(first.calls_async().await, 2);
        second.assert_calls_async(2).await;
    }

    #[test]
    fn exact_duplicate_sources_are_ignored_before_slug_collisions() {
        let config = Config::parse(
            "[[sources]]\nurl = \"https://example.com/feed.xml\"\nname = \"first\"\n\
             [[sources]]\nurl = \"https://example.com/feed.xml\"\nname = \"second\"\n",
        )
        .unwrap();
        let sources = config.resolve_sources(&no_env).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name.as_deref(), Some("first"));
    }

    #[test]
    fn unresolved_local_sources_require_loading_the_config() {
        let config = Config::parse("[[sources]]\nurl = \"./aggr-ai.toml\"\n").unwrap();
        let err = config.resolve_sources(&no_env).unwrap_err();
        assert!(
            format!("{err:#}").contains("load the config from a file"),
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
                limit: None,
            }
        );
        assert_eq!(sources[1].slug, "x-reads");
        assert_eq!(
            sources[1].engine,
            Engine::Aggr {
                url: Url::parse("https://git.example.com/x/reads.git").unwrap(),
                branch: "data".into(),
                sources: vec!["hn".into()],
                limit: Some(5),
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
