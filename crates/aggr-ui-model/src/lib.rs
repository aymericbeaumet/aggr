//! Serializable page contracts shared by the renderer and the reader.

use serde::Serialize;

/// Source identity included in rendered-output cache keys.
pub const IMPLEMENTATION: &str = include_str!("lib.rs");

/// Everything the client needs to render and navigate one page.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientPage {
    /// Relative path from this page to the site root (`../../`); the client resolves it once.
    pub base: String,
    /// Site path of this page, `""` for the root.
    pub path: String,
    /// `river`, `source`, `category`, `tag`, `item`, `browse`, `preferences`, `404`, `offline`, …
    pub kind: String,
    pub title: String,
    pub site: ClientSite,
    pub build: ClientBuild,
    pub page: ClientView,
    pub head: ClientHead,
}

/// Site-wide values, identical on every page of one build.
#[derive(Debug, Clone, Default, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientSite {
    pub title: String,
    pub language: String,
    /// Whether a manifest and service worker were built.
    pub pwa: bool,
    pub hermetic: bool,
    pub preference_rules: std::collections::BTreeMap<&'static str, BootstrapRule>,
    pub discussions: Vec<DiscussionLinkCtx>,
    /// The first nine feed entries, for the `g 1` … `g 9` shortcuts.
    pub entries: Vec<String>,
    /// Browser-facing page for the source config, when the build knows one; the header links
    /// the site's own `aggr.toml` copy otherwise.
    pub config_url: Option<String>,
    pub assets: ClientAssets,
}

/// Site paths of the compiled client, resolved through Vite's manifest at build time.
#[derive(Debug, Clone, Default, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientAssets {
    pub app: Option<String>,
    pub css: Option<String>,
    /// Chunks the entry imports eagerly, for `<link rel="modulepreload">`.
    pub imports: Vec<String>,
    pub icon: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientBuild {
    pub version: String,
    /// Binary release and embedded UI bytes, independent of feed or config data.
    pub app: String,
    /// Semantic content and rendered configuration, independent of rebuild time and app bytes.
    pub content: String,
}

/// The view rendered by the shared Svelte components on the server and in the browser.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "view", content = "data", rename_all = "snake_case")]
pub enum ClientView {
    List(Box<ClientList>),
    Article(Box<ClientArticle>),
    Preferences(Box<ClientPreferences>),
    Offline,
    Directory(Box<ClientDirectory>),
    NotFound(ClientNotFound),
}

#[derive(Debug, Clone, Default, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientNotFound {
    pub data_branch: String,
    pub archive_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientList {
    pub rows: Vec<ClientRow>,
    pub paginator: PaginatorCtx,
    /// The source, category or tag this list is scoped to; `None` on the river.
    pub scope: Option<ClientScope>,
    pub error: Option<ClientSourceError>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientScope {
    /// `source`, `category` or `tag`.
    pub kind: String,
    pub slug: String,
    pub name: String,
    pub site_url: Option<String>,
    /// Readable profile label of the source's URL, shown beside its name.
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientSourceError {
    pub message: String,
    /// RFC 3339.
    pub since: String,
}

/// One row: a feed entry, a related card, an article header or a search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientRow {
    /// `items/<source>/<yyyy>/<mm>/<stem>`, the identity used by tab state and search.
    pub path: String,
    /// Site path of the item page.
    pub url: String,
    pub link: String,
    pub title: String,
    pub language: Option<String>,
    pub excerpt: String,
    pub age_band: String,
    pub category: Option<String>,
    pub labels: Vec<ClientLabel>,
    pub preview: Option<ClientPreview>,
    pub metadata: Metadata,
}

/// A topic label with the slug its tag page and query use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientLabel {
    pub name: String,
    pub slug: String,
}

/// What a row needs to draw a thumbnail. The ThumbHash stays out: the build has already decoded
/// it into `data_url`, and nothing in the reader reads the hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientPreview {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub alt: Option<String>,
    pub color: Option<String>,
    pub placeholder: ClientPlaceholder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientPlaceholder {
    pub data_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientArticle {
    pub header: ClientRow,
    pub authors: Vec<String>,
    pub resources: Vec<ResourceLink>,
    pub access: ClientAccess,
    pub has_margin_notes: bool,
    pub media: Option<ClientMedia>,
    pub lead: Option<ClientLead>,
    pub previous: Option<ClientRow>,
    pub next: Option<ClientRow>,
    pub recommended: Vec<ClientRow>,
}

/// Why an article body may be missing.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientAccess {
    Normal,
    SubscriptionRequired {
        archive_lookup_url: Option<String>,
    },
    TitlesOnly,
    /// A feed kept no article text and no media. The original link is the way to read it.
    Missing,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientPreferences {
    pub schema: PreferenceSchema,
    /// Typed initial browser settings, keyed like preference exports.
    pub values: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct Metadata {
    pub original: String,
    pub date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub updated: Option<String>,
    pub source_slug: String,
    pub source_query: String,
    pub source_display: String,
    pub source_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub category: Option<Category>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub feed_display: Option<String>,
    pub feed_sources: Vec<SourceMembershipCtx>,
    pub is_aggregated: bool,
    pub word_count: usize,
    pub reading_minutes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub consumption: Option<Consumption>,
    pub discussions: Vec<Discussion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub points: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub comments: Option<Comments>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct Category {
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct Discussion {
    pub name: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub score: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct Comments {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct Consumption {
    pub action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub minutes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub words: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct DiscussionLinkCtx {
    pub name: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub shortcut: Option<String>,
    /// A direct link to a matching discussion rather than the provider's search page.
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub score: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct PaginatorCtx {
    /// Maximum number of entries in one pager.
    pub paginate_by: usize,
    /// Route prefix for numbered pagers, e.g. `sources/rust/page/`.
    pub base_url: String,
    /// Number of generated pagers.
    pub number_pagers: usize,
    /// Routes for the two edges. These are always present, including on an edge pager.
    pub first: String,
    pub last: String,
    pub previous: Option<String>,
    pub next: Option<String>,
    /// Current pager, 1-indexed.
    pub current_index: usize,
    /// Number of entries across every pager.
    pub total_items: usize,
    /// Rank of the first entry on this pager, 0-based.
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct SourceMembershipCtx {
    pub slug: String,
    pub query_value: String,
    pub name: String,
    pub display: String,
}

#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreferenceOption {
    pub value: String,
    pub label: String,
}

#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Control {
    Select,
    Number,
    Checkbox,
}

#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BootstrapRule {
    pub initial: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub values: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub min: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub max: Option<i64>,
    /// `documentElement.dataset` key this setting drives, when CSS reacts to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub attribute: Option<&'static str>,
}

#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreferenceField {
    pub key: &'static str,
    pub label: &'static str,
    pub control: Control,
    pub value: serde_json::Value,
    pub options: Vec<PreferenceOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub min: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub max: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub help: Option<&'static str>,
}

#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreferenceGroup {
    pub id: &'static str,
    pub title: &'static str,
    pub fields: Vec<PreferenceField>,
}

#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PreferenceSchema {
    /// Keyed by setting name, for the inline validation bootstrap.
    pub bootstrap: std::collections::BTreeMap<&'static str, BootstrapRule>,
    /// Ordered groups, for the static form.
    pub groups: Vec<PreferenceGroup>,
}

#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceLink {
    pub label: String,
    pub url: String,
}
/// The article body is supplied to the renderer, never copied into the embedded page model.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct RenderRequest {
    pub page: ClientPage,
    pub body_html: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientHead {
    pub title: String,
    pub meta: Vec<ClientMeta>,
    pub links: Vec<ClientLink>,
    pub schema: Option<serde_json::Value>,
    pub bootstrap: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientMeta {
    pub name: Option<String>,
    pub property: Option<String>,
    pub http_equiv: Option<String>,
    pub id: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientLink {
    pub rel: String,
    pub href: String,
    pub title: Option<String>,
    pub mime_type: Option<String>,
    pub sizes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientLead {
    pub url: String,
    pub srcset: String,
    pub width: u32,
    pub height: u32,
    pub alt: Option<String>,
    pub color: Option<String>,
    pub placeholder: ClientPlaceholder,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientMedia {
    Document {
        url: String,
    },
    Video {
        provider: String,
        title: String,
        embed_url: String,
        requires_parent: bool,
    },
    NativeAudio {
        url: String,
    },
    NativeVideo {
        url: String,
    },
    Interactive {
        url: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientDirectory {
    pub groups: Vec<ClientDirectoryGroup>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientDirectoryGroup {
    pub kind: String,
    pub title: String,
    pub entries: Vec<ClientDirectoryEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct ClientDirectoryEntry {
    pub name: String,
    pub url: String,
    pub count: usize,
    pub latest: Option<String>,
    pub site_url: Option<String>,
    pub engine: Option<String>,
    pub category: Option<Category>,
    pub error: Option<ClientSourceError>,
}

#[cfg(feature = "typescript")]
pub fn export_types(path: impl AsRef<std::path::Path>) -> Result<(), ts_rs::ExportError> {
    use ts_rs::TS;
    RenderRequest::export_all(
        &ts_rs::Config::new()
            .with_large_int("number")
            .with_out_dir(path.as_ref()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn article_text_is_not_duplicated_in_the_browser_model() {
        let request = RenderRequest {
            page: ClientPage {
                base: "../".into(),
                path: "article/".into(),
                kind: "item".into(),
                title: "Article".into(),
                site: ClientSite::default(),
                build: ClientBuild::default(),
                page: ClientView::NotFound(ClientNotFound::default()),
                head: ClientHead::default(),
            },
            body_html: "<p>One archived body</p>".into(),
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["body_html"], "<p>One archived body</p>");
        assert!(!value["page"].to_string().contains("One archived body"));
        assert_eq!(value["page"]["page"]["view"], "not_found");
    }

    #[cfg(feature = "typescript")]
    #[test]
    fn typescript_contract_includes_the_renderer_and_all_page_variants() {
        use ts_rs::TS;
        let config = ts_rs::Config::new().with_large_int("number");
        let views = ClientView::decl(&config);
        for view in [
            "list",
            "article",
            "preferences",
            "offline",
            "directory",
            "not_found",
        ] {
            assert!(views.contains(view), "missing view {view}");
        }
        assert!(!Consumption::decl(&config).contains("bigint"));
        assert!(!BootstrapRule::decl(&config).contains("bigint"));
        let dependencies = RenderRequest::dependencies(&config);
        assert!(
            dependencies
                .iter()
                .any(|dependency| dependency.ts_name == "ClientPage")
        );
    }
}
