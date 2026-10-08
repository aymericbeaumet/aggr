//! `aggr.toml`: on-disk schema, defaults, validation, and resolution into engine-ready sources.

mod import_formats;
mod import_graph;
pub(crate) mod language;
mod network_config;
mod policy;
pub mod preferences;
pub(crate) mod repository_url;
mod source_entries;

pub use network_config::NetworkConfig;
pub use policy::{Defaults, LimitOverrides, Limits, MediaPolicy};
pub use preferences::ReaderPreferences;
use source_entries::deserialize_sources;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use url::Url;

pub const DEFAULT_FILE: &str = "aggr.toml";

/// Every option with its default value, commented. Shipped as `config.default.toml` and written
/// by `aggr init --defaults`.
pub const DEFAULTS: &str = include_str!("../config.default.toml");

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub defaults: Defaults,
    pub site: SiteConfig,
    /// Initial reader settings. A visitor's saved preferences override these; Reset restores them.
    pub preferences: ReaderPreferences,
    /// Optional public identity attached to the site's Schema.org metadata.
    pub identity: Option<SiteIdentityConfig>,
    pub store: StoreConfig,
    pub fetch: FetchConfig,
    pub cache: CacheConfig,
    #[serde(
        default,
        rename = "networks",
        deserialize_with = "network_config::reject_legacy_networks"
    )]
    #[allow(dead_code)]
    networks: (),
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
    pub items_per_page: usize,
    /// Items shown in the recent home feed. Source/category/tag archives remain complete.
    pub max_items: usize,
    pub max_age_days: u32,
    /// `owner/repo`; defaults to `$GITHUB_REPOSITORY` when unset.
    pub repository: Option<String>,
    /// Public URL of the site (`--release` builds). A custom domain here also writes `CNAME`.
    pub url: Option<Url>,
    /// Opt into search-engine indexing and sitemaps for release builds.
    pub indexing: bool,
    /// Maximum logical bytes in a successful generated site, including search and assets.
    pub build_max_bytes: u64,
    /// Publication-age window whose retained media keeps its full quality in the site.
    pub media_full_quality_days: u32,
    pub out: PathBuf,
    /// Emit install metadata and cache a bounded offline set in a secure context.
    pub pwa: bool,
    #[serde(
        default,
        rename = "preferences",
        deserialize_with = "preferences::reject_legacy_site_preferences"
    )]
    #[allow(dead_code)]
    pub(crate) legacy_preferences: (),
    #[serde(
        default,
        rename = "identity",
        deserialize_with = "reject_legacy_site_identity"
    )]
    #[allow(dead_code)]
    pub(crate) legacy_identity: (),
    #[serde(default, rename = "params", deserialize_with = "reject_site_params")]
    #[allow(dead_code)]
    pub(crate) legacy_params: (),
}

fn reject_legacy_site_identity<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde::de::IgnoredAny::deserialize(deserializer)?;
    Err(serde::de::Error::custom(
        "`[site.identity]` has moved to `[identity]`",
    ))
}

fn reject_site_params<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde::de::IgnoredAny::deserialize(deserializer)?;
    Err(serde::de::Error::custom("`[site.params]` has been removed"))
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
            items_per_page: 50,
            max_items: 5000,
            max_age_days: 365,
            repository: None,
            url: None,
            indexing: false,
            build_max_bytes: 1_000_000_000,
            media_full_quality_days: 30,
            out: PathBuf::from("_site"),
            pwa: true,
            legacy_preferences: (),
            legacy_identity: (),
            legacy_params: (),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum NetworkProvider {
    HackerNews,
    Reddit,
    X,
}

impl NetworkProvider {
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "hackernews" => Some(Self::HackerNews),
            "reddit" => Some(Self::Reddit),
            "x" => Some(Self::X),
            _ => None,
        }
    }

    pub(crate) fn network(self) -> NetworkConfig {
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
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            branch: "aggr".into(),
            dir: PathBuf::from(".aggr/data"),
            html: true,
            html_max_bytes: 262_144,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FetchConfig {
    pub concurrency: usize,
    /// Concurrent original-article downloads within each source whose
    /// `content_download_strategy` is `article`.
    pub article_concurrency: usize,
    pub timeout_secs: u64,
    pub max_body_bytes: usize,
    pub retries: u32,
    /// Allow a remote collection to expand collections outside its own origin or repository.
    pub allow_remote_source_chains: bool,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            concurrency: 16,
            article_concurrency: 4,
            timeout_secs: 20,
            max_body_bytes: 10_000_000,
            retries: 2,
            allow_remote_source_chains: false,
        }
    }
}

/// `feed` keeps the feed entry. `article` also downloads and extracts the original page.
#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum ContentMode {
    #[serde(rename = "article")]
    Heavy,
    #[default]
    #[serde(rename = "feed")]
    Light,
}

/// Limits for disposable local state, independent of archive retention and site size.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CacheConfig {
    pub media_max_bytes: u64,
    pub response_max_bytes: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            media_max_bytes: 2 * 1024 * 1024 * 1024,
            response_max_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PreviewPolicy {
    #[default]
    Remote,
    Local,
    Disabled,
}

impl PreviewPolicy {
    pub fn archives(self) -> bool {
        self == Self::Local
    }
    pub fn remote(self) -> bool {
        self == Self::Remote
    }
    pub fn enabled(self) -> bool {
        self != Self::Disabled
    }
}

impl<'de> Deserialize<'de> for PreviewPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Value {
            Boolean(bool),
            Name(String),
        }
        match Value::deserialize(deserializer)? {
            Value::Boolean(true) => Ok(Self::Local),
            Value::Boolean(false) => Ok(Self::Disabled),
            Value::Name(name) => match name.as_str() {
                "remote" => Ok(Self::Remote),
                "local" => Ok(Self::Local),
                "disabled" => Ok(Self::Disabled),
                _ => Err(serde::de::Error::custom(
                    "previews must be remote, local, disabled, or a boolean",
                )),
            },
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DocumentPolicy {
    #[default]
    Remote,
    Original,
}

impl DocumentPolicy {
    pub fn archives(self) -> bool {
        self == Self::Original
    }
}

impl<'de> Deserialize<'de> for DocumentPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Value {
            Boolean(bool),
            Name(String),
        }
        match Value::deserialize(deserializer)? {
            Value::Boolean(true) => Ok(Self::Original),
            Value::Boolean(false) => Ok(Self::Remote),
            Value::Name(name) => match name.as_str() {
                "remote" => Ok(Self::Remote),
                "original" => Ok(Self::Original),
                _ => Err(serde::de::Error::custom(
                    "documents must be remote, original, or a boolean",
                )),
            },
        }
    }
}

/// What aggr does with an article's images: leave them at the publisher, archive exactly what the
/// publisher served, or archive one bounded copy of it. The level a compact copy is reduced to
/// belongs to that mode alone, so no other mode can be given one.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ImagePolicy {
    /// `"remote"` (or `false`): nothing is downloaded and the reader loads the publisher's URL.
    #[default]
    Remote,
    /// `"original"` (or `true`): the publisher's exact bytes.
    Original,
    /// `"compact"`: one bounded JPEG master and no renditions. Images dominate an archive's size,
    /// so this is the difference between gigabytes and hundreds of megabytes; what it gives up is
    /// the publisher's exact bytes, which no later run can recover.
    Compact(crate::media::CompactPolicy),
}

impl ImagePolicy {
    /// Whether article images are archived at all under this policy.
    pub fn archives(self) -> bool {
        self != Self::Remote
    }

    /// The bounds a newly archived image is reduced to, if any.
    pub fn compaction(self) -> Option<crate::media::CompactPolicy> {
        match self {
            Self::Compact(policy) => Some(policy),
            Self::Remote | Self::Original => None,
        }
    }
}

impl Serialize for ImagePolicy {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Remote => serializer.serialize_str("remote"),
            Self::Original => serializer.serialize_str("original"),
            Self::Compact(policy) => {
                use serde::ser::SerializeStruct;
                let mut value = serializer.serialize_struct("ImagePolicy", 3)?;
                value.serialize_field("mode", "compact")?;
                value.serialize_field("quality", &policy.jpeg_quality)?;
                value.serialize_field("max_axis", &policy.max_axis)?;
                value.end()
            }
        }
    }
}

/// `images` is one key with three shapes: a boolean, a mode name, or a compact mode with the
/// level it is reduced to. Each shape is visited directly so a mistyped key reports itself
/// instead of collapsing into "matched no variant".
impl<'de> Deserialize<'de> for ImagePolicy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Tuned {
            mode: String,
            quality: Option<u8>,
            max_axis: Option<u32>,
        }

        struct Policy;

        impl<'de> serde::de::Visitor<'de> for Policy {
            type Value = ImagePolicy;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(
                    "\"remote\", \"original\", \"compact\", a boolean, or a table naming a mode",
                )
            }

            // The booleans that configurations already carry keep meaning what they meant.
            fn visit_bool<E: serde::de::Error>(self, archive: bool) -> Result<Self::Value, E> {
                Ok(if archive {
                    ImagePolicy::Original
                } else {
                    ImagePolicy::Remote
                })
            }

            fn visit_str<E: serde::de::Error>(self, mode: &str) -> Result<Self::Value, E> {
                resolve(mode, None, None).map_err(E::custom)
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                map: M,
            ) -> Result<Self::Value, M::Error> {
                let tuned = Tuned::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                resolve(&tuned.mode, tuned.quality, tuned.max_axis)
                    .map_err(serde::de::Error::custom)
            }
        }

        fn resolve(
            mode: &str,
            quality: Option<u8>,
            max_axis: Option<u32>,
        ) -> Result<ImagePolicy, String> {
            let leveled = quality.is_some() || max_axis.is_some();
            match mode {
                "remote" | "original" if leveled => Err(format!(
                    "images mode {mode:?} has nothing to tune; \"quality\" and \"max_axis\" belong to \"compact\""
                )),
                "remote" => Ok(ImagePolicy::Remote),
                "original" => Ok(ImagePolicy::Original),
                "compact" => {
                    let default = crate::media::CompactPolicy::archive();
                    let jpeg_quality = quality.unwrap_or(default.jpeg_quality);
                    let max_axis = max_axis.unwrap_or(default.max_axis);
                    if !(1..=100).contains(&jpeg_quality) {
                        return Err(format!("images quality {jpeg_quality} is outside 1-100"));
                    }
                    if !(MIN_COMPACT_AXIS..=MAX_COMPACT_AXIS).contains(&max_axis) {
                        return Err(format!(
                            "images max_axis {max_axis} is outside {MIN_COMPACT_AXIS}-{MAX_COMPACT_AXIS}"
                        ));
                    }
                    Ok(ImagePolicy::Compact(crate::media::CompactPolicy {
                        max_axis,
                        jpeg_quality,
                    }))
                }
                other => Err(format!(
                    "unknown images mode {other:?}; expected \"remote\", \"original\" or \"compact\""
                )),
            }
        }

        deserializer.deserialize_any(Policy)
    }
}

/// A compact master still has to be worth reading on a wide screen, and still has to stay inside
/// the decoder bounds every archived image is held to.
const MIN_COMPACT_AXIS: u32 = 320;
const MAX_COMPACT_AXIS: u32 = 8_192;

/// One normalized source candidate, expanded from a `[[sources]]` group.
/// Engine-specific options are validated when it resolves to a Source.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourceConfig {
    /// Internal dispatch hint after URL and environment resolution.
    #[serde(skip)]
    pub(crate) kind: Option<String>,
    pub url: Option<String>,
    /// Inspect an opaque URL as a subscription collection. Omitted inherits `[defaults]`.
    /// The flag applies to this entry only and is not copied onto expanded feeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<bool>,
    /// Local feed documents are produced only by imports, never by deserializing a URL.
    #[serde(skip)]
    pub(crate) local_feed: Option<PathBuf>,
    /// Display name; the upstream feed title is used when unset.
    pub name: Option<String>,
    /// Directory name under `items/`; derived from `name` or `url` when unset.
    pub slug: Option<String>,
    pub category: Option<String>,
    /// Labels applied to every item from this source. Feed-provided labels are appended.
    /// Default labels are added in front; a source cannot remove them.
    pub labels: Vec<String>,
    /// Extra request headers; values support `${ENV}` expansion.
    pub headers: BTreeMap<String, String>,
    pub html: Option<bool>,
    /// Override `[defaults] content_download_strategy` for this source.
    #[serde(rename = "content_download_strategy")]
    pub content: Option<ContentMode>,
    #[serde(rename = "media_download_strategy")]
    pub media: Option<MediaPolicy>,
    #[serde(flatten)]
    pub limits: LimitOverrides,
    /// Repository source: data branch of that repository.
    pub branch: Option<String>,
    /// Repository source: only these of its sources. Omitted inherits `[defaults]`;
    /// an empty list keeps every source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<String>>,
    /// Replaces `[defaults] discussions` for items from this source. An empty list disables them.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "network_config::deserialize_optional_discussions"
    )]
    pub discussions: Option<Vec<NetworkConfig>>,
    #[serde(
        default,
        skip_serializing,
        rename = "networks",
        deserialize_with = "network_config::reject_legacy_networks"
    )]
    #[allow(dead_code)]
    pub(crate) networks: (),
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
    pub previews: PreviewPolicy,
    pub images: ImagePolicy,
    pub documents: DocumentPolicy,
    pub limits: Limits,
    /// Discussion searches for items from this source, after defaults and overrides.
    pub discussions: Vec<NetworkConfig>,
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
        for source in &mut config.sources {
            if source.collection.is_none() {
                source.collection = config.defaults.collection;
            }
        }
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
        self.preferences.validate()?;
        if self.cache.media_max_bytes == 0 || self.cache.response_max_bytes == 0 {
            bail!("[cache] byte limits must be positive");
        }
        if self
            .site
            .description
            .as_deref()
            .is_some_and(|description| description.trim().is_empty())
        {
            bail!("[site] description must not be empty");
        }
        if self
            .identity
            .as_ref()
            .is_some_and(|identity| identity.name.trim().is_empty())
        {
            bail!("[identity] name must not be empty");
        }
        if !language::is_well_formed(&self.site.language) {
            bail!("[site] language must be a BCP 47 tag such as `en` or `fr-FR`");
        }
        if self.site.items_per_page == 0 {
            bail!("[site] items_per_page must be at least 1");
        }
        if self.site.build_max_bytes == 0 {
            bail!("[site] build_max_bytes must be at least 1");
        }
        if self.fetch.concurrency == 0 {
            bail!("[fetch] concurrency must be at least 1");
        }
        if self.fetch.article_concurrency == 0 {
            bail!("[fetch] article_concurrency must be at least 1");
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
        if let Some(slug) = &self.defaults.slug {
            validate_slug(slug).context("[defaults] slug")?;
        }
        for slug in &self.defaults.sources {
            validate_slug(slug).context("[defaults] sources")?;
        }
        Ok(())
    }

    /// Expand every `[[sources]]` table into an engine-ready [`Source`], reading `${ENV}`
    /// references from the process environment.
    pub fn sources(&self) -> Result<Vec<Source>> {
        self.resolve_sources(&|name| std::env::var(name).ok())
    }

    pub(crate) fn resolve_source(&self, raw: &SourceConfig) -> Result<Source> {
        resolve_source(raw, &self.defaults, self.store.html, &|name| {
            std::env::var(name).ok()
        })
    }

    pub fn resolve_sources(&self, env: &dyn Fn(&str) -> Option<String>) -> Result<Vec<Source>> {
        let mut seen_slugs = BTreeSet::new();
        let mut seen_identities = BTreeMap::<String, (usize, String)>::new();
        let mut sources = Vec::with_capacity(self.sources.len());
        for (index, raw) in self.sources.iter().enumerate() {
            let mut source = resolve_source(raw, &self.defaults, self.store.html, env)
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
            // A derived slug names the publisher, so several feeds from one host arrive at the
            // same name. Tell them apart with the path that differs rather than making the reader
            // name each one by hand. An explicit duplicate is still a mistake worth reporting.
            // A slug inherited from `[defaults]` is explicit: every source that omitted one
            // shares it, and that collision is reported. Derived slugs are still disambiguated.
            if raw.slug.is_none() && self.defaults.slug.is_none() {
                source.slug = distinct_slug(&source.slug, source.engine.url(), &seen_slugs);
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

fn describe(raw: &SourceConfig) -> String {
    raw.slug
        .clone()
        .or_else(|| raw.name.clone())
        .or_else(|| raw.url.clone())
        .unwrap_or_else(|| "<empty>".into())
}

fn validate_source_options(raw: &SourceConfig) -> Result<()> {
    let filter = raw.sources.as_deref().unwrap_or_default();
    let aggr_keys = [
        ("branch", raw.branch.is_some()),
        ("sources", !filter.is_empty()),
    ];
    let only = |owner: &str, keys: &[(&str, bool)]| -> Result<()> {
        for (key, set) in keys {
            if *set {
                bail!("`{key}` only applies to {owner} repository sources");
            }
        }
        Ok(())
    };
    match source_kind(raw) {
        "feed" => only("aggr", &aggr_keys)?,
        "aggr" => {
            for slug in filter {
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

fn source_kind(raw: &SourceConfig) -> &str {
    raw.kind.as_deref().unwrap_or_else(|| {
        if raw.url.as_deref().is_some_and(repository_url::inferred) {
            "aggr"
        } else {
            "feed"
        }
    })
}

fn present(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

struct Inherited<'a> {
    url: &'a Option<String>,
    name: &'a Option<String>,
    slug: &'a Option<String>,
    category: &'a Option<String>,
    labels: &'a [String],
    headers: &'a BTreeMap<String, String>,
    html: Option<bool>,
    discussions: Option<&'a [NetworkConfig]>,
}

impl<'a> Inherited<'a> {
    fn from_defaults(defaults: &'a Defaults) -> Self {
        Self {
            url: &defaults.url,
            name: &defaults.name,
            slug: &defaults.slug,
            category: &defaults.category,
            labels: &defaults.labels,
            headers: &defaults.headers,
            html: defaults.html,
            discussions: Some(defaults.discussions.as_slice()),
        }
    }

    fn from_source(source: &'a SourceConfig) -> Self {
        Self {
            url: &source.url,
            name: &source.name,
            slug: &source.slug,
            category: &source.category,
            labels: &source.labels,
            headers: &source.headers,
            html: source.html,
            discussions: source.discussions.as_deref(),
        }
    }

    fn from_document(defaults: &'a policy::DocumentDefaults) -> Self {
        Self {
            url: &defaults.url,
            name: &defaults.name,
            slug: &defaults.slug,
            category: &defaults.category,
            labels: &defaults.labels,
            headers: &defaults.headers,
            html: defaults.html,
            discussions: defaults.discussions.as_deref(),
        }
    }
}

impl SourceConfig {
    pub(crate) fn is_collection(&self) -> bool {
        self.collection == Some(true)
    }

    /// Root `[defaults]` presentation. Repository fields wait until the URL has been expanded
    /// and the source kind is known.
    fn inherit_defaults(&mut self, defaults: &Defaults) {
        self.fill_presentation(Inherited::from_defaults(defaults));
    }

    fn inherit_repository(&mut self, defaults: &Defaults) {
        let sources = (!defaults.sources.is_empty()).then(|| defaults.sources.clone());
        self.inherit_repository_from(defaults.branch.clone(), sources);
    }

    /// Parent `[[sources]]` table. `collection` stays on the entry that declared it.
    fn inherit_source(&mut self, parent: &Self) {
        self.fill_presentation(Inherited::from_source(parent));
        self.inherit_repository_from(parent.branch.clone(), parent.sources.clone());
    }

    fn inherit_document(&mut self, defaults: &policy::DocumentDefaults) {
        let sources = (!defaults.sources.is_empty()).then(|| defaults.sources.clone());
        self.fill_presentation(Inherited::from_document(defaults));
        self.inherit_repository_from(defaults.branch.clone(), sources);
    }

    fn fill_presentation(&mut self, inherited: Inherited<'_>) {
        self.url = present(self.url.take()).or_else(|| present(inherited.url.clone()));
        self.name = present(self.name.take()).or_else(|| present(inherited.name.clone()));
        self.slug = present(self.slug.take()).or_else(|| present(inherited.slug.clone()));
        self.category =
            present(self.category.take()).or_else(|| present(inherited.category.clone()));
        if !inherited.labels.is_empty() {
            let mut merged = inherited.labels.to_vec();
            merged.append(&mut self.labels);
            self.labels = merged;
        }
        if !inherited.headers.is_empty() {
            self.headers = policy::merge_headers(inherited.headers, &self.headers);
        }
        self.html = self.html.or(inherited.html);
        if self.discussions.is_none()
            && let Some(discussions) = inherited.discussions
        {
            self.discussions = Some(discussions.to_vec());
        }
    }

    /// `branch` and `sources` are aggr repository options. An omitted filter inherits; an empty
    /// list is an explicit "keep every source" and is left alone.
    fn inherit_repository_from(&mut self, branch: Option<String>, sources: Option<Vec<String>>) {
        if source_kind(self) != "aggr" {
            return;
        }
        self.branch = self.branch.take().or(branch);
        if self.sources.is_none() {
            self.sources = sources;
        }
    }
}

/// Shortcut help lists each enabled discussion once. With no sources yet, the root list is the
/// only one a reader could turn on.
pub(crate) fn enabled_discussions(
    defaults: &[NetworkConfig],
    sources: &[Source],
) -> Vec<NetworkConfig> {
    if sources.is_empty() {
        return defaults.to_vec();
    }
    let mut seen = BTreeSet::new();
    let mut enabled = Vec::new();
    for source in sources {
        for discussion in &source.discussions {
            let key = format!(
                "{:?}\0{}\0{}",
                discussion.provider, discussion.name, discussion.url
            );
            if seen.insert(key) {
                enabled.push(discussion.clone());
            }
        }
    }
    enabled
}

fn resolve_source(
    raw: &SourceConfig,
    defaults: &Defaults,
    default_html: bool,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Source> {
    let mut raw = raw.clone();
    raw.inherit_defaults(defaults);
    if raw.kind.is_none()
        && let Some(url) = raw.url.as_deref()
        && repository_url::inferred(&expand_env(url, env)?)
    {
        raw.kind = Some("aggr".into());
    }
    raw.inherit_repository(defaults);
    let kind = source_kind(&raw);
    let identity = source_identity(&raw, kind);
    validate_source_options(&raw)?;
    let engine = match kind {
        "feed" => Engine::Feed {
            url: match &raw.local_feed {
                Some(path) => Url::from_file_path(path)
                    .map_err(|_| anyhow::anyhow!("invalid local feed path"))?,
                None => http_url(&raw, env)?,
            },
        },
        "aggr" => {
            let url = repository_url::parse(&expand_env(
                raw.url.as_deref().context("`url` is required")?,
                env,
            )?)?;
            Engine::Aggr {
                url,
                branch: raw.branch.clone().unwrap_or_else(|| "aggr".into()),
                sources: raw.sources.clone().unwrap_or_default(),
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
        public_url: effective_url.and_then(|url| match &engine {
            Engine::Aggr { .. } => repository_url::public(url, !persist_endpoint),
            Engine::Feed { .. } => {
                matches!(url.scheme(), "http" | "https").then(|| public_url(url, !persist_endpoint))
            }
        }),
        persist_endpoint,
        headers,
        html: raw.html.unwrap_or(default_html),
        content: raw.content.unwrap_or(defaults.content),
        previews: raw.media.unwrap_or(defaults.media).previews(),
        images: raw.media.unwrap_or(defaults.media).images(),
        documents: raw.media.unwrap_or(defaults.media).documents(),
        limits: raw.limits.resolve(defaults.limits),
        discussions: raw.discussions.clone().unwrap_or_default(),
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
    let repository = (kind == "aggr")
        .then(|| raw.url.as_deref().and_then(repository_url::identity))
        .flatten();
    crate::model::sha1_hex(format!(
        "source-v2\0{kind}\0{}\0\0{}\0{}\0{headers}",
        repository
            .as_deref()
            .or(raw.url.as_deref())
            .unwrap_or_default(),
        raw.branch
            .as_deref()
            .unwrap_or(if kind == "aggr" { "aggr" } else { "" }),
        raw.sources.as_deref().unwrap_or_default().join(",")
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

/// Derive a directory-safe slug from the display name, else from the source's canonical name
/// (`https://www.example.com/blog/feed.xml` → `example-com`, and a platform account keeps the
/// path that names it: `https://youtube.com/@Alice` → `youtube-com-alice`).
pub fn derive_slug(name: Option<&str>, url: Option<&Url>) -> String {
    if let Some(name) = name {
        let slug = slug::slugify(name);
        if !slug.is_empty() {
            return truncate_slug(&slug);
        }
    }
    let slug = url
        .and_then(crate::platform::canonical_name)
        .map(slug::slugify)
        .unwrap_or_default();
    if slug.is_empty() {
        "source".into()
    } else {
        truncate_slug(&slug)
    }
}

/// Path segments that describe the transport rather than the publisher.
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

/// Keep a derived slug unique: first by adding the feed path that distinguishes it from a
/// sibling on the same host, then by counting.
fn distinct_slug(slug: &str, url: Option<&Url>, taken: &BTreeSet<String>) -> String {
    if !taken.contains(slug) {
        return slug.to_string();
    }
    if let Some(url) = url {
        let path = url
            .path_segments()
            .into_iter()
            .flatten()
            .filter(|segment| !segment.is_empty() && !is_feed_noise(segment))
            .collect::<Vec<_>>()
            .join("-");
        let extended = truncate_slug(&slug::slugify(format!("{slug} {path}")));
        if !extended.is_empty() && extended != slug && !taken.contains(&extended) {
            return extended;
        }
    }
    (2..)
        .map(|suffix| truncate_slug(&format!("{slug}-{suffix}")))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| slug.to_string())
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
        assert!(!config.site.indexing);
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
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private/reader/feeds/news.xml");
        for name in [None, Some(""), Some("   "), Some("!!!")] {
            let config = Config {
                sources: vec![SourceConfig {
                    local_feed: Some(path.clone()),
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
                "'''\n  https://one.example/feed#part # first\n # comment\n  https://two.example/ # second\n'''",
                r#"["https://one.example/feed#part # first", "https://two.example/ # second"]"#,
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
[defaults]
content_download_strategy = "feed"
discussions = ["reddit"]
[[sources]]
url = ["https://two.example/", "https://three.example/"]
category = "second"
[[sources]]
url = "https://one.example/"
name = "Duplicate"
[[sources]]
url = "https://${HOST}/"
headers = { Authorization = "Bearer ${TOKEN}" }
media_download_strategy = "remote"
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
        assert!(!sources[3].images.archives());
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
    async fn ordinary_remote_sources_resolve_without_any_preflight_requests() {
        use httpmock::prelude::*;
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let preflight = server
            .mock_async(|when, then| {
                when.method(GET);
                then.status(500);
            })
            .await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aggr.toml");
        let urls = [
            server.url("/"),
            server.url("/feed.xml"),
            server.url("/rss"),
            server.url("/feed.json"),
            server.url("/opaque-subscriptions"),
        ];
        std::fs::write(&path, format!("[[sources]]\nurl={urls:?}\n")).unwrap();
        let config = Config::load(&path).await.unwrap();
        assert_eq!(config.sources.len(), urls.len());
        assert!(config.loaded_remote.is_empty());
        preflight.assert_calls_async(0).await;
        for (source, expected) in config.sources.iter().zip(urls) {
            assert_eq!(source.url.as_deref(), Some(expected.as_str()));
        }
    }

    #[tokio::test]
    async fn explicit_opaque_collections_expand_once_without_fetching_their_leaves() {
        use httpmock::prelude::*;
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let collection = server
            .mock_async(|when, then| {
                when.method(GET).path("/subscriptions");
                then.status(200)
                    .body("[[sources]]\nurl=['./feed', './page']\nlabels=['leaf']\n");
            })
            .await;
        let leaves = server
            .mock_async(|when, then| {
                when.method(GET).path_matches("/(feed|page)$");
                then.status(500);
            })
            .await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aggr.toml");
        std::fs::write(
            &path,
            format!(
                "[[sources]]\nurl={:?}\ncollection=true\ncategory='news'\n",
                server.url("/subscriptions")
            ),
        )
        .unwrap();
        let config = Config::load(&path).await.unwrap();
        assert_eq!(config.sources.len(), 2);
        assert_eq!(config.loaded_remote.len(), 1);
        for source in &config.sources {
            assert!(!source.is_collection());
            assert_eq!(source.category.as_deref(), Some("news"));
            assert_eq!(source.labels, ["leaf"]);
        }
        collection.assert_calls_async(1).await;
        leaves.assert_calls_async(0).await;
    }

    #[tokio::test]
    async fn url_collections_and_remote_chain_setting_expand_sources() {
        crate::http::install_crypto_provider();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aggr.toml");
        std::fs::write(
            &root,
            "[fetch]\nallow_remote_source_chains = true\n[[sources]]\nurl = './topics.toml'\ncategory = 'Programming'\n",
        ).unwrap();
        std::fs::write(
            dir.path().join("topics.toml"),
            "[[sources]]\nurl = './nested.toml'\n",
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
    fn rejects_removed_configuration_aliases() {
        for text in [
            "[[sources]]\ninclude = './a.toml'",
            "[[sources]]\nrepo = 'friend/reads'",
            "[[sources]]\ntype = 'aggr'\nurl = 'https://github.com/friend/reads'",
            "[fetch]\nallow_remote_include_chains = true",
            "[site]\noffline_items = 30",
            "[site]\nmax_stubs = 20000",
            "[[networks]]\nprovider = 'hn'",
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
        let err = Config::parse("[site]\ntheme = \"default\"\n").unwrap_err();
        assert!(err.to_string().contains("theme"), "{err}");
        let err = Config::parse("[[sources]]\nurl = \"https://a.b/c\"\nfoo = 1\n").unwrap_err();
        assert!(err.to_string().contains("foo"), "{err}");
    }

    #[test]
    fn source_engines_are_inferred_from_urls() {
        for kind in ["feed", "aggr", "html", "telegram"] {
            let err = Config::parse(&format!(
                "[[sources]]\ntype={kind:?}\nurl='https://example.org/feed'\n"
            ))
            .unwrap_err();
            assert!(err.to_string().contains("type"), "{err:#}");
        }
        assert!(
            Config::parse("[[sources]]\nurl='https://example.org/feed'\nitems='li'\n").is_err()
        );
    }

    #[test]
    fn requires_url_for_feed() {
        let config = Config::parse("[[sources]]\nname = \"x\"\n").unwrap();
        let err = config.resolve_sources(&no_env).unwrap_err();
        assert!(format!("{err:#}").contains("`url` is required"), "{err:#}");
    }

    #[test]
    fn derived_slugs_stay_distinct_and_explicit_duplicates_are_rejected() {
        // Two feeds from one publisher share a canonical name, so the path that differs names
        // them apart; a feed endpoint carries no identity, so the second falls back to counting.
        let config = Config::parse(
            "[[sources]]\nurl = \"https://example.com/blog/feed.xml\"\n\
             [[sources]]\nurl = \"https://example.com/notes/feed.xml\"\n\
             [[sources]]\nurl = \"https://www.example.com/rss\"\n",
        )
        .unwrap();
        let slugs: Vec<_> = config
            .resolve_sources(&no_env)
            .unwrap()
            .into_iter()
            .map(|source| source.slug)
            .collect();
        assert_eq!(slugs, ["example-com", "example-com-notes", "example-com-2"]);

        // A slug the reader set by hand twice is a mistake, not something to paper over.
        let config = Config::parse(
            "[[sources]]\nslug = \"mine\"\nurl = \"https://a.example/feed.xml\"\n\
             [[sources]]\nslug = \"mine\"\nurl = \"https://b.example/feed.xml\"\n",
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
        // An ordinary site is named by its domain: the feed's path says nothing more.
        assert_eq!(
            derive_slug(None, Some(&url("https://blog.rust-lang.org/feed.xml"))),
            "blog-rust-lang-org"
        );
        assert_eq!(
            derive_slug(None, Some(&url("https://www.example.com/blog/feed/"))),
            "example-com"
        );
        assert_eq!(
            derive_slug(None, Some(&url("https://hnrss.org/frontpage"))),
            "hnrss-org"
        );
        // A platform shares one host between publishers, so the account's path names the source.
        assert_eq!(
            derive_slug(
                None,
                Some(&url("https://github.com/rust-lang/rust/releases.atom"))
            ),
            "github-com-rust-lang"
        );
        assert_eq!(
            derive_slug(None, Some(&url("https://www.youtube.com/@SomeChannel"))),
            "youtube-com-somechannel"
        );
        assert_eq!(
            derive_slug(None, Some(&url("https://www.reddit.com/r/rust/.rss"))),
            "reddit-com-r-rust"
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
        assert_eq!(config.site.items_per_page, compiled.site.items_per_page);
        assert_eq!(compiled.site.items_per_page, 50);
        assert_eq!(config.site.max_items, compiled.site.max_items);
        assert_eq!(config.site.max_age_days, compiled.site.max_age_days);
        assert_eq!(config.site.repository, compiled.site.repository);
        assert_eq!(config.site.url, compiled.site.url);
        assert_eq!(config.site.indexing, compiled.site.indexing);
        assert_eq!(config.site.build_max_bytes, compiled.site.build_max_bytes);
        assert_eq!(
            config.site.media_full_quality_days,
            compiled.site.media_full_quality_days
        );
        assert_eq!(config.site.out, compiled.site.out);
        assert_eq!(config.site.pwa, compiled.site.pwa);
        assert_eq!(config.identity, compiled.identity);
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
        assert_eq!(config.fetch.max_body_bytes, compiled.fetch.max_body_bytes);
        assert_eq!(config.fetch.retries, compiled.fetch.retries);
        assert_eq!(
            config.fetch.allow_remote_source_chains,
            compiled.fetch.allow_remote_source_chains
        );
        assert_eq!(config.defaults, compiled.defaults);
        assert!(compiled.defaults.discussions.is_empty());
        assert!(config.sources.is_empty());
    }

    #[test]
    fn the_shipped_example_config_resolves() {
        // `examples/aggr.toml` backs `make run` and the docs; keep it loadable as written.
        let config = Config::parse(include_str!("../examples/aggr.toml")).unwrap();
        assert_eq!(config.site.title, "My reads");
        let sources = config.resolve_sources(&no_env).unwrap();
        assert_eq!(sources.len(), 6);
        for source in &sources {
            assert!(source.name.is_some(), "{} has no name", source.slug);
            assert!(source.category.is_some(), "{} has no category", source.slug);
            let url = source
                .engine
                .url()
                .unwrap_or_else(|| panic!("{} resolved without an engine URL", source.slug));
            assert_eq!(url.scheme(), "https", "{}", source.slug);
        }
        let categories = sources
            .iter()
            .filter_map(|source| source.category.as_deref())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            categories.into_iter().collect::<Vec<_>>(),
            ["fun", "news", "rust", "tools"]
        );
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
    fn source_policy_roundtrip_preserves_inheritance_and_explicit_clear() {
        let config = Config::parse("[defaults]\ncontent_download_strategy='article'\nmedia_download_strategy='original'\n[[sources]]\nurl='https://example.com/feed'\nmedia_download_strategy={mode='compressed',quality=65}\nsince=false\ncount_max=0\n").unwrap();
        let encoded = serde_json::to_string(&config.sources[0]).unwrap();
        let decoded: SourceConfig = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.media, config.sources[0].media);
        assert_eq!(decoded.limits, config.sources[0].limits);
        let source = config.sources().unwrap().remove(0);
        assert_eq!(source.content, ContentMode::Heavy);
        assert_eq!(source.previews, PreviewPolicy::Local);
        assert_eq!(source.documents, DocumentPolicy::Original);
        assert_eq!(source.images.compaction().unwrap().jpeg_quality, 65);
        for invalid in [
            "[cache]\nmedia_max_bytes=0",
            "[cache]\nresponse_max_bytes=0",
        ] {
            assert!(Config::parse(invalid).is_err());
        }
    }

    #[test]
    fn source_html_inherits_store_default_and_can_override_it() {
        let config = Config::parse("[store]\nhtml=false\n[[sources]]\nurl='https://a.example/feed'\n[[sources]]\nurl='https://b.example/feed'\nhtml=true\n").unwrap();
        let sources = config.sources().unwrap();
        assert!(!sources[0].html);
        assert!(sources[1].html);
    }

    #[test]
    fn site_description_and_identity_are_configurable() {
        let config = Config::parse(
            r#"
[site]
description = "Independent reading notes and useful links."

[identity]
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
        let identity = config.identity.unwrap();
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
            Config::parse("[identity]\ntype = \"organization\"\nname = \" \"\n").unwrap_err();
        assert!(
            empty_identity
                .to_string()
                .contains("[identity] name must not be empty"),
            "{empty_identity}"
        );
        let moved =
            Config::parse("[site.identity]\ntype = \"person\"\nname = \"Ada\"\n").unwrap_err();
        assert!(
            moved
                .to_string()
                .contains("`[site.identity]` has moved to `[identity]`"),
            "{moved}"
        );
        let removed = Config::parse("[site.params]\nintroduction = true\n").unwrap_err();
        assert!(
            removed
                .to_string()
                .contains("`[site.params]` has been removed"),
            "{removed}"
        );
    }

    #[test]
    fn site_language_requires_a_well_formed_bcp_47_tag() {
        // The grammar itself is covered next to `language::is_well_formed`.
        assert!(Config::parse("[site]\nlanguage = \"fr-FR\"\n").is_ok());
        let error = Config::parse("[site]\nlanguage = \"en_US\"\n").unwrap_err();
        assert!(error.to_string().contains("BCP 47"), "{error}");
    }

    #[test]
    fn discussions_accept_provider_names_and_custom_tables() {
        let config = Config::parse(
            r#"
[defaults]
discussions = [
  "hackernews",
  { provider = "reddit" },
  { name = "Lobsters", url = "https://lobste.rs/search?q={url}" },
]
"#,
        )
        .unwrap();
        assert_eq!(config.defaults.discussions.len(), 3);
        assert_eq!(
            config.defaults.discussions[0],
            NetworkProvider::HackerNews.network()
        );
        assert_eq!(
            config.defaults.discussions[1],
            NetworkProvider::Reddit.network()
        );
        assert_eq!(config.defaults.discussions[2].name, "Lobsters");
        assert_eq!(config.defaults.discussions[2].provider, None);

        for invalid in [
            "[defaults]\ndiscussions = [{ provider = \"hackernews\", name = \"HN\" }]\n",
            "[defaults]\ndiscussions = [{ name = \"Lobsters\" }]\n",
            "[defaults]\ndiscussions = [{ url = \"https://example.com/{url}\" }]\n",
            "[defaults]\ndiscussions = [\"hn\"]\n",
            "[defaults]\ndiscussions = [\"hackernews\", \"hackernews\"]\n",
            "[[site.discussions]]\nprovider = \"hackernews\"\n",
        ] {
            assert!(Config::parse(invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn legacy_networks_key_points_at_discussions() {
        for text in [
            "[[networks]]\nprovider = \"hackernews\"\n",
            "[defaults]\nnetworks = [\"hackernews\"]\n",
            "[[sources]]\nurl = \"https://example.com/feed\"\nnetworks = [\"reddit\"]\n",
        ] {
            let error = Config::parse(text).unwrap_err();
            assert!(error.to_string().contains("discussions"), "{error}");
        }
        let Err(error) = import_formats::parse_bytes(
            b"[[networks]]\nprovider = \"reddit\"\n[[sources]]\nurl = \"https://example.com/feed\"\n",
            &Url::parse("https://example.com/feeds.toml").unwrap(),
        ) else {
            panic!("imported networks were accepted");
        };
        assert!(format!("{error:#}").contains("discussions"), "{error:#}");
    }

    #[test]
    fn source_options_inherit_from_defaults_and_sources_can_override_them() {
        let config = Config::parse(
            r#"
[store]
html = true

[defaults]
url = "https://default.example/feed"
name = "Default"
category = "news"
labels = ["root"]
headers = { Accept = "root", X-Root = "1" }
html = false
discussions = ["hackernews", "reddit"]

[[sources]]
labels = ["leaf"]
headers = { Accept = "leaf" }

[[sources]]
url = "https://other.example/feed"
name = "Other"
discussions = []

[[sources]]
url = "https://reddit.example/feed"
name = "Reddit only"
discussions = ["reddit"]
labels = ["only"]
"#,
        )
        .unwrap();
        let sources = config.resolve_sources(&no_env).unwrap();
        assert_eq!(
            sources[0].public_url.as_deref(),
            Some("https://default.example/feed")
        );
        assert_eq!(sources[0].name.as_deref(), Some("Default"));
        assert_eq!(sources[0].category.as_deref(), Some("news"));
        assert_eq!(sources[0].labels, ["leaf", "root"]);
        assert_eq!(
            sources[0].headers,
            vec![
                ("Accept".into(), "leaf".into()),
                ("X-Root".into(), "1".into()),
            ]
        );
        assert!(!sources[0].html);
        assert_eq!(
            sources[0].discussions,
            vec![
                NetworkProvider::HackerNews.network(),
                NetworkProvider::Reddit.network(),
            ]
        );
        assert_eq!(sources[1].labels, ["root"]);
        assert!(sources[1].discussions.is_empty());
        assert!(!sources[1].html);
        assert_eq!(
            sources[2].discussions,
            vec![NetworkProvider::Reddit.network()]
        );
        assert_eq!(sources[2].labels, ["only", "root"]);
    }

    #[test]
    fn repository_defaults_apply_only_to_aggr_sources() {
        let config = Config::parse(
            r#"
[defaults]
branch = "notes"
sources = ["kept"]

[[sources]]
url = "https://github.com/example/reads"

[[sources]]
url = "https://example.com/feed"

[[sources]]
url = "https://github.com/example/other"
sources = []
"#,
        )
        .unwrap();
        let sources = config.resolve_sources(&no_env).unwrap();
        assert!(
            matches!(&sources[0].engine, Engine::Aggr { branch, sources, .. } if branch == "notes" && sources == &["kept".to_string()])
        );
        assert!(matches!(sources[1].engine, Engine::Feed { .. }));
        assert!(
            matches!(&sources[2].engine, Engine::Aggr { branch, sources, .. } if branch == "notes" && sources.is_empty())
        );
        let cleared: SourceConfig = serde_json::from_str(r#"{"sources":[]}"#).unwrap();
        assert_eq!(cleared.sources, Some(Vec::new()));
    }

    #[test]
    fn explicit_default_slug_must_be_unique_per_source() {
        let error = Config::parse(
            "[defaults]\nslug = \"same\"\n[[sources]]\nurl = \"https://a.example/feed\"\n[[sources]]\nurl = \"https://b.example/feed\"\n",
        )
        .unwrap()
        .resolve_sources(&no_env)
        .unwrap_err();
        assert!(error.to_string().contains("slug"), "{error}");
    }

    #[tokio::test]
    async fn document_defaults_follow_source_and_root_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("outer.toml"),
            "[defaults]\ncategory = \"document\"\nlabels = [\"document\"]\ndiscussions = [\"reddit\"]\n[[sources]]\nurl = \"https://leaf.example/feed\"\nlabels = [\"leaf\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("aggr.toml"),
            "[defaults]\ncategory = \"root\"\nlabels = [\"root\"]\ndiscussions = [\"hackernews\"]\n[[sources]]\nurl = \"./outer.toml\"\ncategory = \"wrapper\"\nlabels = [\"wrapper\"]\n",
        )
        .unwrap();
        let config = Config::load_offline(&root.join("aggr.toml")).await.unwrap();
        let sources = config.sources().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].category.as_deref(), Some("wrapper"));
        assert_eq!(sources[0].labels, ["document", "leaf", "root", "wrapper"]);
        assert_eq!(
            sources[0].discussions,
            vec![NetworkProvider::Reddit.network()]
        );
    }

    #[tokio::test]
    async fn default_collection_expands_only_entries_that_omit_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("feeds.toml"),
            "[[sources]]\nurl = \"https://leaf.example/feed\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("aggr.toml"),
            "[defaults]\ncollection = true\n[[sources]]\nurl = \"./feeds.toml\"\n[[sources]]\nurl = \"https://remote.example/feed\"\ncollection = false\n",
        )
        .unwrap();
        let config = Config::load_offline(&dir.path().join("aggr.toml"))
            .await
            .unwrap();
        let leaf = config
            .sources
            .iter()
            .find(|source| source.url.as_deref() == Some("https://leaf.example/feed"))
            .unwrap();
        assert_eq!(leaf.collection, None);
        let remote = config
            .sources
            .iter()
            .find(|source| source.url.as_deref() == Some("https://remote.example/feed"))
            .unwrap();
        assert_eq!(remote.collection, Some(false));
        let sources = config.sources().unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(
            sources[0].public_url.as_deref(),
            Some("https://leaf.example/feed")
        );
        assert_eq!(
            sources[1].public_url.as_deref(),
            Some("https://remote.example/feed")
        );
    }

    #[test]
    fn feed_content_is_default_and_sources_can_choose_article() {
        let config = Config::parse(
            "[[sources]]\nurl = \"https://heavy.example/feed\"\n\
             [[sources]]\nurl = \"https://light.example/feed\"\ncontent_download_strategy = \"article\"\n",
        )
        .unwrap();
        let sources = config.sources().unwrap();
        assert_eq!(sources[0].content, ContentMode::Light);
        assert_eq!(sources[1].content, ContentMode::Heavy);
        assert!(Config::parse("[defaults]\ncontent_download_strategy = \"medium\"\n").is_err());
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
            "[[sources]]\nurl = \"https://github.com/owner/reading/aggr.toml\"\ncategory = \"shared\"\n",
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
        forbidden.assert_calls_async(0).await;
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
        assert_eq!(
            second.calls_async().await,
            0,
            "a forbidden hop is refused before it costs a request"
        );

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
        second.assert_calls_async(1).await;
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
            "[[sources]]\nurl = \"https://github.com/friend/reads\"\ncategory = \"friends\"\n\n\
             [[sources]]\nurl = \"https://git.example.com/x/reads.git\"\nbranch = \"data\"\nsources = [\"hn\"]\ncount_max = 5\n",
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
            }
        );
        assert_eq!(sources[1].slug, "x-reads");
        assert_eq!(
            sources[1].engine,
            Engine::Aggr {
                url: Url::parse("https://git.example.com/x/reads.git").unwrap(),
                branch: "data".into(),
                sources: vec!["hn".into()],
            }
        );
        assert_eq!(sources[1].limits.max_items, 5);

        for text in [
            "[[sources]]\nurl='https://github.com/friend/reads'\nlimit=0",
            "[[sources]]\nurl='https://example.org/feed'\nbranch='archive'",
            "[[sources]]\nurl='https://github.com/friend/reads'\nsources=['INVALID']",
        ] {
            assert!(
                Config::parse(text)
                    .and_then(|config| config.resolve_sources(&no_env))
                    .is_err(),
                "{text}"
            );
        }
    }

    #[test]
    fn repository_urls_infer_aggr_and_share_identity() {
        let urls = [
            "https://github.com/friend/reads",
            "https://github.com/friend/reads.git/",
            "git@github.com:friend/reads.git",
            "ssh://git@github.com/friend/reads.git",
        ];
        let mut identities = BTreeSet::new();
        for url in urls {
            let config = Config::parse(&format!("[[sources]]\nurl={url:?}\ncategory=' Friends '\nbranch='archive'\nsources=['news']\ncount_max=5")).unwrap();
            let sources = config.resolve_sources(&no_env).unwrap();
            assert_eq!(sources[0].slug, "friend-reads");
            assert_eq!(sources[0].category.as_deref(), Some("friends"));
            assert_eq!(
                sources[0].public_url.as_deref(),
                Some("https://github.com/friend/reads")
            );
            assert!(
                matches!(&sources[0].engine, Engine::Aggr { branch, sources, .. } if branch == "archive" && sources == &["news"])
            );
            assert_eq!(sources[0].limits.max_items, 5);
            identities.insert(sources[0].identity.clone());
        }
        assert_eq!(identities.len(), 1);
        let config = Config::parse(&format!("[[sources]]\nurl={urls:?}")).unwrap();
        assert_eq!(config.resolve_sources(&no_env).unwrap().len(), 1);
    }

    #[test]
    fn repository_inference_preserves_feed_and_explicit_collection_urls() {
        for url in [
            "https://github.com/friend/reads/releases.atom",
            "https://github.com/friend/reads/blob/main/aggr.toml",
            "https://example.org/posts/article",
        ] {
            let config = Config::parse(&format!("[[sources]]\nurl={url:?}")).unwrap();
            assert!(matches!(
                config.resolve_sources(&no_env).unwrap()[0].engine,
                Engine::Feed { .. }
            ));
        }
        for url in [
            "git@git.example.org:team/reads.git",
            "ssh://git@git.example.org:2222/team/reads.git",
            "https://git.example.org/team/reads.git",
        ] {
            let config = Config::parse(&format!("[[sources]]\nurl={url:?}")).unwrap();
            assert!(matches!(
                config.resolve_sources(&no_env).unwrap()[0].engine,
                Engine::Aggr { .. }
            ));
        }
    }
}
