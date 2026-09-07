//! Safe, deterministic expansion of local and remote aggr configuration graphs.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use url::Url;

use super::{FetchConfig, SourceConfig};
use crate::http::{self, Request, Response};

const MAX_COLLECTION_DEPTH: usize = 32;
const MAX_SOURCE_DOCUMENTS: usize = 4096;

pub(super) struct Expansion {
    pub sources: Vec<SourceConfig>,
    pub local: Vec<PathBuf>,
    pub remote: Vec<(String, String)>,
}

pub(super) async fn expand(
    sources: Vec<SourceConfig>,
    root: PathBuf,
    fetch: &FetchConfig,
    github_api: Option<Url>,
    load_remote: bool,
) -> Result<Expansion> {
    let client = http::Client::new(fetch)?;
    let root = root
        .canonicalize()
        .with_context(|| format!("resolving {}", root.display()))?;
    let root_location = Location::Local(root.clone());
    let mut loader = Loader {
        client,
        load_remote,
        allow_remote_chains: fetch.allow_remote_source_chains,
        github_api,
        seen: BTreeSet::from([root_location.key()]),
        local: vec![root],
        remote: Vec::new(),
        default_branches: BTreeMap::new(),
        trees: BTreeMap::new(),
        documents: 0,
    };
    let sources = loader.expand_sources(sources, root_location, None, 0).await;
    Ok(Expansion {
        sources,
        local: loader.local,
        remote: loader.remote,
    })
}

struct Loader {
    client: http::Client,
    load_remote: bool,
    allow_remote_chains: bool,
    github_api: Option<Url>,
    seen: BTreeSet<String>,
    local: Vec<PathBuf>,
    remote: Vec<(String, String)>,
    default_branches: BTreeMap<String, String>,
    trees: BTreeMap<String, Vec<String>>,
    documents: usize,
}

impl Loader {
    fn expand_sources(
        &mut self,
        sources: Vec<SourceConfig>,
        declaring: Location,
        defaults: Option<SourceConfig>,
        depth: usize,
    ) -> Pin<Box<dyn Future<Output = Vec<SourceConfig>> + Send + '_>> {
        Box::pin(async move {
            let mut expanded = Vec::new();
            for mut source in sources {
                let explicit_headers = source.headers.clone();
                if let Some(defaults) = &defaults {
                    apply_defaults(&mut source, defaults);
                }
                if source.kind.as_deref() == Some("aggr") || source.local_feed.is_some() {
                    expanded.push(source);
                    continue;
                }
                let Some(pattern) = source.url.clone() else {
                    expanded.push(source);
                    continue;
                };
                let env = |name: &str| std::env::var(name).ok();
                let resolved = match super::expand_env(&pattern, &env) {
                    Ok(value) => value,
                    Err(_) => {
                        expanded.push(source);
                        continue;
                    }
                };
                if !self.load_remote && remote_url(&resolved).ok().flatten().is_some() {
                    expanded.push(source);
                    continue;
                }
                let targets = match self.targets(&declaring, &resolved).await {
                    Ok(targets) => targets,
                    Err(err) => {
                        warn(
                            &format!("source {} from {declaring}", import_label(&pattern)),
                            &err,
                        );
                        continue;
                    }
                };
                for target in targets {
                    let mut source = source.clone();
                    if declaring.is_remote() && !declaring.same_origin(&target) {
                        source.headers = explicit_headers.clone();
                    }
                    let headers = source
                        .headers
                        .iter()
                        .map(|(name, value)| {
                            super::expand_env(value, &env).map(|value| (name.clone(), value))
                        })
                        .collect::<Result<Vec<_>>>();
                    let headers = match headers {
                        Ok(headers) => headers,
                        Err(_) => {
                            expanded.push(source);
                            continue;
                        }
                    };
                    let requested = target.clone();
                    if self.documents >= MAX_SOURCE_DOCUMENTS {
                        log::warn!(
                            "source expansion reached its limit of {MAX_SOURCE_DOCUMENTS} documents"
                        );
                        return expanded;
                    }
                    let key = target.key();
                    if self.seen.contains(&key) {
                        log::warn!("ignoring repeated or cyclic source {target}");
                        continue;
                    }
                    let remote = target.is_remote();
                    let absolute = remote_url(&resolved).ok().flatten().is_some();
                    let original_url = if absolute {
                        Some(pattern.clone())
                    } else if let Location::Generic(url) = &target {
                        Some(url.to_string())
                    } else {
                        None
                    };
                    let generic = matches!(target, Location::Generic(_));
                    self.documents += 1;
                    let (document, actual, digest) = match self.read(target, &headers).await {
                        Ok(value) => value,
                        Err(err) => {
                            if remote && !err.is::<super::import_formats::InvalidCollection>() {
                                // Feed and site failures remain ordinary runtime source failures.
                                let mut leaf = source.clone();
                                leaf.url = original_url.clone().or_else(|| Some(pattern.clone()));
                                expanded.push(leaf);
                            } else {
                                warn(
                                    &format!("source {} from {declaring}", import_label(&pattern)),
                                    &err,
                                );
                            }
                            continue;
                        }
                    };
                    if document.collection {
                        if !requested.same_origin(&actual) && requested.is_remote() {
                            source.headers.clear();
                        }
                        if self.seen.contains(&actual.key()) {
                            log::warn!(
                                "ignoring repeated or cyclic redirected collection {actual}"
                            );
                            continue;
                        }
                        if depth >= MAX_COLLECTION_DEPTH {
                            log::warn!(
                                "ignoring collection {actual}: maximum nesting depth {MAX_COLLECTION_DEPTH} exceeded"
                            );
                            continue;
                        }
                        if declaring.is_remote()
                            && !declaring.same_origin(&actual)
                            && !self.allow_remote_chains
                        {
                            log::warn!(
                                "ignoring remote collection {actual}: cross-origin source chains require [fetch] allow_remote_source_chains = true"
                            );
                            continue;
                        }
                        self.seen.insert(key);
                        self.seen.insert(actual.key());
                        if let Some(digest) = digest {
                            self.remote.push((actual.key(), digest));
                        }
                        let nested = self
                            .expand_sources(
                                document.sources,
                                actual,
                                Some(source.clone()),
                                depth + 1,
                            )
                            .await;
                        expanded.extend(nested);
                    } else {
                        for mut leaf in document.sources {
                            apply_defaults(&mut leaf, &source);
                            // Keep unresolved credentials in the configured URL for stable identity.
                            if generic {
                                leaf.url = original_url.clone();
                            }
                            expanded.push(leaf);
                        }
                    }
                }
            }
            expanded
        })
    }

    async fn targets(&mut self, declaring: &Location, pattern: &str) -> Result<Vec<Location>> {
        if pattern.trim().is_empty() {
            bail!("`url` must name a source or collection");
        }
        if pattern.to_ascii_lowercase().starts_with("file:") {
            if declaring.is_remote() {
                bail!("remote collections cannot read local file URLs");
            }
            let path = Url::parse(pattern)?
                .to_file_path()
                .map_err(|_| anyhow::anyhow!("invalid local file URL"))?;
            let path = path.canonicalize().context("resolving local file URL")?;
            return Ok(vec![Location::Local(path)]);
        }
        if let Some(url) = remote_url(pattern)? {
            return self.remote_targets(url).await;
        }
        match declaring {
            Location::Local(file) => {
                local_targets(file.parent().unwrap_or_else(|| Path::new(".")), pattern)
            }
            Location::Generic(url) => {
                if has_glob(pattern) {
                    bail!(
                        "wildcards need an enumerable repository URL (GitHub is supported); \
                         plain HTTP cannot list {pattern:?}"
                    );
                }
                if pattern.starts_with("//") {
                    bail!("a remote config may only import a relative path on its own host");
                }
                let joined = url.join(pattern).with_context(|| {
                    format!(
                        "resolving {} against {}",
                        import_label(pattern),
                        safe_url(url)
                    )
                })?;
                if joined.origin() != url.origin() {
                    bail!("a remote config may only import a relative path on its own host");
                }
                Ok(vec![Location::Generic(joined)])
            }
            Location::GitHub(file) => {
                let path = repository_path(&file.path, pattern)?;
                self.github_targets(file.repo.clone(), path).await
            }
        }
    }

    async fn remote_targets(&mut self, url: Url) -> Result<Vec<Location>> {
        if let Some(location) = github_location(&url, self.github_api.as_ref())? {
            return self.github_targets(location.repo, location.path).await;
        }
        if has_glob(url.path()) {
            bail!(
                "wildcards need an enumerable repository URL (GitHub is supported); plain HTTP \
                 cannot list {}",
                safe_url(&url)
            );
        }
        Ok(vec![Location::Generic(url)])
    }

    async fn github_targets(
        &mut self,
        mut repo: GitHubRepo,
        path: String,
    ) -> Result<Vec<Location>> {
        if repo.reference.is_none() {
            repo.reference = Some(self.default_branch(&repo).await?);
        }
        if !has_glob(&path) {
            return Ok(vec![Location::GitHub(GitHubFile { repo, path })]);
        }
        let reference = repo
            .reference
            .clone()
            .context("GitHub reference was not resolved")?;
        let paths = self.tree(&repo, &reference).await?;
        let pattern = glob::Pattern::new(&path)
            .with_context(|| format!("invalid remote import glob {path:?}"))?;
        let options = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: true,
        };
        let mut matched: Vec<_> = paths
            .iter()
            .filter(|candidate| pattern.matches_path_with(Path::new(candidate), options))
            .cloned()
            .collect();
        matched.sort();
        if matched.is_empty() {
            bail!("remote import glob {path:?} matched no file in {repo}");
        }
        Ok(matched
            .into_iter()
            .map(|path| {
                Location::GitHub(GitHubFile {
                    repo: repo.clone(),
                    path,
                })
            })
            .collect())
    }

    async fn default_branch(&mut self, repo: &GitHubRepo) -> Result<String> {
        let key = repo.key();
        if let Some(branch) = self.default_branches.get(&key) {
            return Ok(branch.clone());
        }
        #[derive(Deserialize)]
        struct Metadata {
            default_branch: String,
        }
        let url = repo.api_url(&[])?;
        let bytes = self.github_get(&url, "application/vnd.github+json").await?;
        let metadata: Metadata = serde_json::from_slice(&bytes)
            .with_context(|| format!("reading repository metadata for {repo}"))?;
        if metadata.default_branch.is_empty() {
            bail!("GitHub returned an empty default branch for {repo}");
        }
        self.default_branches
            .insert(key, metadata.default_branch.clone());
        Ok(metadata.default_branch)
    }

    async fn tree(&mut self, repo: &GitHubRepo, reference: &str) -> Result<Vec<String>> {
        let key = format!("{}@{reference}", repo.key());
        if let Some(paths) = self.trees.get(&key) {
            return Ok(paths.clone());
        }
        #[derive(Deserialize)]
        struct Tree {
            #[serde(default)]
            tree: Vec<Entry>,
            #[serde(default)]
            truncated: bool,
        }
        #[derive(Deserialize)]
        struct Entry {
            path: String,
            #[serde(rename = "type")]
            kind: String,
        }
        let mut url = repo.api_url(&["git", "trees", reference])?;
        url.query_pairs_mut().append_pair("recursive", "1");
        let bytes = self.github_get(&url, "application/vnd.github+json").await?;
        let tree: Tree = serde_json::from_slice(&bytes)
            .with_context(|| format!("reading repository tree for {repo}@{reference}"))?;
        if tree.truncated {
            bail!("GitHub truncated the tree for {repo}@{reference}; narrow the import pattern");
        }
        let paths: Vec<_> = tree
            .tree
            .into_iter()
            .filter(|entry| entry.kind == "blob")
            .map(|entry| entry.path)
            .collect();
        self.trees.insert(key, paths.clone());
        Ok(paths)
    }

    async fn read(
        &mut self,
        location: Location,
        headers: &[(String, String)],
    ) -> Result<(super::import_formats::Document, Location, Option<String>)> {
        match location {
            Location::Local(path) => {
                let limit = self.client.max_body_bytes();
                let mut bytes = Vec::new();
                std::fs::File::open(&path)
                    .with_context(|| format!("reading {}", path.display()))?
                    .take(limit.saturating_add(1) as u64)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > limit {
                    bail!("source document exceeds {limit} bytes");
                }
                let url = Url::from_file_path(&path)
                    .map_err(|_| anyhow::anyhow!("invalid local import path {}", path.display()))?;
                let sources = super::import_formats::parse_bytes(&bytes, &url)
                    .with_context(|| format!("parsing import {}", path.display()))?;
                self.local.push(path.clone());
                Ok((sources, Location::Local(path), None))
            }
            Location::Generic(url) => {
                let body = self.get(&url, headers).await?;
                let actual = Location::Generic(body.final_url.clone());
                let sources = super::import_formats::parse_bytes(&body.bytes, &body.final_url)
                    .with_context(|| format!("parsing import {}", safe_url(&url)))?;
                let digest = sources
                    .collection
                    .then(|| crate::model::sha1_hex(&body.bytes));
                Ok((sources, actual, digest))
            }
            Location::GitHub(file) => {
                let mut url = file.repo.api_url(&["contents"])?;
                {
                    let mut segments = url
                        .path_segments_mut()
                        .map_err(|_| anyhow::anyhow!("GitHub API URL cannot hold path segments"))?;
                    for segment in file.path.split('/').filter(|segment| !segment.is_empty()) {
                        segments.push(segment);
                    }
                }
                if let Some(reference) = &file.repo.reference {
                    url.query_pairs_mut().append_pair("ref", reference);
                }
                let bytes = self
                    .github_get(&url, "application/vnd.github.raw+json")
                    .await?;
                let source_url = file.raw_url()?;
                let sources = super::import_formats::parse_bytes(&bytes, &source_url)
                    .with_context(|| format!("parsing import {file}"))?;
                let location = Location::GitHub(file);
                let digest = sources.collection.then(|| crate::model::sha1_hex(&bytes));
                Ok((sources, location, digest))
            }
        }
    }

    async fn github_get(&self, url: &Url, accept: &str) -> Result<Vec<u8>> {
        let mut headers = vec![("Accept".to_string(), accept.to_string())];
        if let Some(token) = std::env::var("GH_TOKEN")
            .ok()
            .filter(|token| !token.is_empty())
            .or_else(|| {
                std::env::var("GITHUB_TOKEN")
                    .ok()
                    .filter(|token| !token.is_empty())
            })
        {
            headers.push(("Authorization".to_string(), format!("Bearer {token}")));
        }
        self.get(url, &headers).await.map(|body| body.bytes)
    }

    async fn get(&self, url: &Url, headers: &[(String, String)]) -> Result<http::Body> {
        match self
            .client
            .get(Request {
                url,
                headers,
                etag: None,
                last_modified: None,
            })
            .await
            .with_context(|| format!("fetching {}", safe_url(url)))?
        {
            Response::Ok(body) => Ok(body),
            Response::NotModified => {
                bail!("unexpected not-modified response from {}", safe_url(url))
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Location {
    Local(PathBuf),
    Generic(Url),
    GitHub(GitHubFile),
}

impl Location {
    fn same_origin(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Generic(left), Self::Generic(right)) => left.origin() == right.origin(),
            (Self::GitHub(left), Self::GitHub(right)) => left.repo.key() == right.repo.key(),
            _ => false,
        }
    }

    fn is_remote(&self) -> bool {
        !matches!(self, Self::Local(_))
    }

    fn key(&self) -> String {
        match self {
            Self::Local(path) => format!("file:{}", path.to_string_lossy().replace('\\', "/")),
            Self::Generic(url) => format!(
                "url:{}:{}",
                safe_url(url),
                crate::model::sha1_hex(url.as_str())
            ),
            Self::GitHub(file) => format!("github:{file}"),
        }
    }
}

impl std::fmt::Display for Location {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(path) => path.display().fmt(formatter),
            Self::Generic(url) => safe_url(url).fmt(formatter),
            Self::GitHub(file) => file.fmt(formatter),
        }
    }
}

#[derive(Clone, Debug)]
struct GitHubFile {
    repo: GitHubRepo,
    path: String,
}

impl GitHubFile {
    fn raw_url(&self) -> Result<Url> {
        let mut url = Url::parse("https://raw.githubusercontent.com/")?;
        let reference = self
            .repo
            .reference
            .as_deref()
            .context("GitHub reference was not resolved")?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("raw GitHub URL cannot hold path segments"))?
            .extend([self.repo.owner.as_str(), self.repo.name.as_str(), reference])
            .extend(self.path.split('/'));
        Ok(url)
    }
}

impl std::fmt::Display for GitHubFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.repo, self.path)
    }
}

#[derive(Clone, Debug)]
struct GitHubRepo {
    owner: String,
    name: String,
    reference: Option<String>,
    api: Url,
}

impl GitHubRepo {
    fn key(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    fn api_url(&self, tail: &[&str]) -> Result<Url> {
        let mut url = self.api.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("GitHub API URL cannot hold path segments"))?;
            segments.pop_if_empty();
            segments.extend(["repos", self.owner.as_str(), self.name.as_str()]);
            segments.extend(tail.iter().copied());
        }
        Ok(url)
    }
}

impl std::fmt::Display for GitHubRepo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "github.com/{}/{}", self.owner, self.name)
    }
}

fn remote_url(value: &str) -> Result<Option<Url>> {
    let lower = value.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        if value.contains("://") {
            bail!("remote import {value:?} must use http or https");
        }
        return Ok(None);
    }
    if value.chars().any(char::is_whitespace) {
        bail!("remote source URL cannot contain whitespace");
    }
    let Ok(url) = Url::parse(value) else {
        bail!("remote import {value:?} is not a valid URL");
    };
    Ok(Some(url))
}

fn github_location(url: &Url, api_override: Option<&Url>) -> Result<Option<GitHubFile>> {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let parts: Vec<_> = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect();
    let api = match api_override {
        Some(api) => api.clone(),
        None => Url::parse("https://api.github.com/").context("parsing the GitHub API URL")?,
    };
    let (owner, name, reference, path) = match host.as_str() {
        "github.com" if parts.len() >= 2 => {
            let owner = parts[0];
            let name = parts[1].trim_end_matches(".git");
            let rest = &parts[2..];
            match rest {
                [view @ ("blob" | "tree"), reference, tail @ ..] => {
                    let mut path = tail.join("/");
                    if path.is_empty()
                        || (*view == "tree" && !has_glob(&path) && !path.ends_with(".toml"))
                    {
                        path = format!("{}/aggr.toml", path.trim_end_matches('/'))
                            .trim_start_matches('/')
                            .to_string();
                    }
                    (owner, name, Some((*reference).to_string()), path)
                }
                [] => (owner, name, None, "aggr.toml".to_string()),
                tail => (owner, name, None, tail.join("/")),
            }
        }
        "raw.githubusercontent.com" if parts.len() >= 3 => {
            let path = parts.get(3..).unwrap_or_default().join("/");
            (
                parts[0],
                parts[1].trim_end_matches(".git"),
                Some(parts[2].to_string()),
                if path.is_empty() {
                    "aggr.toml".to_string()
                } else {
                    path
                },
            )
        }
        "github.com" | "raw.githubusercontent.com" => {
            bail!(
                "GitHub import {} must identify owner/repository",
                safe_url(url)
            )
        }
        _ => return Ok(None),
    };
    if owner.is_empty() || name.is_empty() {
        bail!(
            "GitHub import {} must identify owner/repository",
            safe_url(url)
        );
    }
    Ok(Some(GitHubFile {
        repo: GitHubRepo {
            owner: owner.to_string(),
            name: name.to_string(),
            reference,
            api,
        },
        path: normalize_repository_path(&path)?,
    }))
}

fn repository_path(declaring: &str, relative: &str) -> Result<String> {
    if relative.starts_with('/') {
        return normalize_repository_path(relative.trim_start_matches('/'));
    }
    let parent = declaring.rsplit_once('/').map_or("", |(parent, _)| parent);
    let path = if parent.is_empty() {
        relative.to_string()
    } else {
        format!("{parent}/{relative}")
    };
    normalize_repository_path(&path)
}

fn normalize_repository_path(path: &str) -> Result<String> {
    let mut components = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => components.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => {
                if components.pop().is_none() {
                    bail!("remote import {path:?} escapes its repository");
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                bail!("remote import {path:?} is not a repository-relative path")
            }
        }
    }
    if components.is_empty() {
        bail!("remote import path is empty");
    }
    Ok(components.join("/"))
}

fn local_targets(dir: &Path, pattern: &str) -> Result<Vec<Location>> {
    let full = dir.join(pattern);
    let mut paths = if has_glob(pattern) {
        let pattern = full.to_string_lossy();
        glob::glob(&pattern)
            .context("invalid local import glob")?
            .filter_map(|entry| entry.ok())
            .filter(|path| path.is_file())
            .collect::<Vec<_>>()
    } else if full.is_file() {
        vec![full]
    } else {
        bail!("{} does not exist or is not a file", full.display());
    };
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        bail!(
            "import glob {pattern:?} matched no file under {}",
            dir.display()
        );
    }
    paths
        .into_iter()
        .map(|path| {
            path.canonicalize()
                .with_context(|| format!("resolving {}", path.display()))
                .map(Location::Local)
        })
        .collect()
}

fn has_glob(value: &str) -> bool {
    value.contains(['*', '?', '['])
}

fn safe_url(url: &Url) -> String {
    super::public_url(url, false)
}

fn import_label(value: &str) -> String {
    remote_url(value)
        .ok()
        .flatten()
        .map_or_else(|| format!("{value:?}"), |url| safe_url(&url))
}

fn warn(context: &str, error: &anyhow::Error) {
    log::warn!("ignoring {context}: {error:#}");
}

fn apply_defaults(source: &mut SourceConfig, defaults: &SourceConfig) {
    source.kind = source.kind.take().or_else(|| defaults.kind.clone());
    source.name = source.name.take().or_else(|| defaults.name.clone());
    source.slug = source.slug.take().or_else(|| defaults.slug.clone());
    source.category = source
        .category
        .take()
        .filter(|value| !value.is_empty())
        .or_else(|| defaults.category.clone());
    let mut labels = defaults.labels.clone();
    labels.append(&mut source.labels);
    source.labels = labels;
    let mut headers = defaults.headers.clone();
    for (name, value) in std::mem::take(&mut source.headers) {
        headers.retain(|default, _| !default.eq_ignore_ascii_case(&name));
        headers.insert(name, value);
    }
    source.headers = headers;
    source.html = source.html.or(defaults.html);
    source.content = source.content.or(defaults.content);
    source.previews = source.previews.or(defaults.previews);
    source.images = source.images.or(defaults.images);
    source.repo = source.repo.take().or_else(|| defaults.repo.clone());
    source.branch = source.branch.take().or_else(|| defaults.branch.clone());
    if source.sources.is_empty() {
        source.sources = defaults.sources.clone();
    }
    source.limit = source.limit.or(defaults.limit);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn collection_depth_and_document_count_are_bounded() {
        crate::http::install_crypto_provider();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aggr.toml");
        std::fs::write(&root, "").unwrap();
        for index in 0..=MAX_COLLECTION_DEPTH + 1 {
            std::fs::write(
                dir.path().join(format!("{index}.toml")),
                format!("[[sources]]\nurl = '{}.toml'\n", index + 1),
            )
            .unwrap();
        }
        let expansion = expand(
            vec![SourceConfig {
                url: Some("0.toml".into()),
                ..Default::default()
            }],
            root.clone(),
            &FetchConfig::default(),
            None,
            false,
        )
        .await
        .unwrap();
        assert!(expansion.sources.is_empty());
        assert_eq!(expansion.local.len(), MAX_COLLECTION_DEPTH + 2);
        let feed = dir.path().join("feed.json");
        std::fs::write(
            &feed,
            r#"{"version":"https://jsonfeed.org/version/1.1","title":"News","items":[]}"#,
        )
        .unwrap();
        let source = SourceConfig {
            url: Some(Url::from_file_path(feed).unwrap().to_string()),
            ..Default::default()
        };
        let expansion = expand(
            vec![source; MAX_SOURCE_DOCUMENTS + 1],
            root,
            &FetchConfig::default(),
            None,
            false,
        )
        .await
        .unwrap();
        assert_eq!(expansion.sources.len(), MAX_SOURCE_DOCUMENTS);
    }

    #[tokio::test]
    async fn collection_headers_stay_on_the_configured_origin() {
        use httpmock::prelude::*;
        crate::http::install_crypto_provider();
        let trusted = MockServer::start();
        let other = MockServer::start();
        let feed = r#"{"version":"https://jsonfeed.org/version/1.1","title":"News","items":[]}"#;
        let collection = trusted.mock(|when, then| {
            when.path("/collection")
                .header("Authorization", "Bearer secret");
            then.status(200).body(format!(
                "[[sources]]\nurl = ['./feed', {:?}, {:?}]\n",
                other.url("/feed"),
                other.url("/collection")
            ));
        });
        let local_feed = trusted.mock(|when, then| {
            when.path("/feed").header("Authorization", "Bearer secret");
            then.status(200).body(feed);
        });
        let leaked = other.mock(|when, then| {
            when.header("Authorization", "Bearer secret");
            then.status(403);
        });
        let remote_feed = other.mock(|when, then| {
            when.path("/feed").header_missing("Authorization");
            then.status(200).body(feed);
        });
        let rejected_collection = other.mock(|when, then| {
            when.path("/collection").header_missing("Authorization");
            then.status(200).body("[[sources]]\nurl = './nested'\n");
        });
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aggr.toml");
        std::fs::write(&root, "").unwrap();
        let expansion = expand(
            vec![SourceConfig {
                url: Some(trusted.url("/collection")),
                headers: BTreeMap::from([("Authorization".into(), "Bearer secret".into())]),
                category: Some("science".into()),
                ..Default::default()
            }],
            root,
            &FetchConfig::default(),
            None,
            true,
        )
        .await
        .unwrap();
        leaked.assert_calls(0);
        collection.assert_calls(1);
        local_feed.assert_calls(1);
        remote_feed.assert_calls(1);
        rejected_collection.assert_calls(1);
        assert_eq!(expansion.sources.len(), 2);
        assert_eq!(expansion.sources[0].headers.len(), 1);
        assert!(expansion.sources[1].headers.is_empty());
        assert_eq!(expansion.sources[1].category.as_deref(), Some("science"));
    }

    #[tokio::test]
    async fn redirected_collections_do_not_forward_original_headers_to_children() {
        use httpmock::prelude::*;
        crate::http::install_crypto_provider();
        let trusted = MockServer::start();
        let other = MockServer::start();
        trusted.mock(|when, then| {
            when.path("/collection")
                .header("Authorization", "Bearer secret");
            then.status(302)
                .header("Location", other.url("/collection"));
        });
        let leaked = other.mock(|when, then| {
            when.header("Authorization", "Bearer secret");
            then.status(403);
        });
        other.mock(|when, then| {
            when.path("/collection").header_missing("Authorization");
            then.status(200).body("[[sources]]\nurl = './feed'\n");
        });
        let feed = other.mock(|when, then| {
            when.path("/feed").header_missing("Authorization");
            then.status(200).body(
                r#"{"version":"https://jsonfeed.org/version/1.1","title":"News","items":[]}"#,
            );
        });
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aggr.toml");
        std::fs::write(&root, "").unwrap();
        let expansion = expand(
            vec![SourceConfig {
                url: Some(trusted.url("/collection")),
                headers: BTreeMap::from([("Authorization".into(), "Bearer secret".into())]),
                ..Default::default()
            }],
            root,
            &FetchConfig::default(),
            None,
            true,
        )
        .await
        .unwrap();
        leaked.assert_calls(0);
        feed.assert_calls(1);
        assert_eq!(expansion.sources.len(), 1);
        assert!(expansion.sources[0].headers.is_empty());
    }

    #[tokio::test]
    async fn redirects_do_not_reexpand_a_seen_collection_and_remote_files_are_rejected() {
        use httpmock::prelude::*;
        crate::http::install_crypto_provider();
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/collection");
            then.status(200)
                .body("[[sources]]\nurl = ['./feed', './alias', 'file:///tmp/private-feed.xml']\n");
        });
        server.mock(|when, then| {
            when.method(GET).path("/alias");
            then.status(302).header("location", "/collection");
        });
        let feed = server.mock(|when, then| {
            when.method(GET).path("/feed");
            then.status(200).body(
                r#"{"version":"https://jsonfeed.org/version/1.1","title":"News","items":[]}"#,
            );
        });
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aggr.toml");
        std::fs::write(&root, "").unwrap();
        let expansion = expand(
            vec![SourceConfig {
                url: Some(server.url("/collection")),
                ..Default::default()
            }],
            root,
            &FetchConfig::default(),
            None,
            true,
        )
        .await
        .unwrap();
        assert_eq!(expansion.sources.len(), 1);
        assert_eq!(expansion.remote.len(), 1);
        feed.assert_calls(1);
    }

    #[tokio::test]
    async fn offline_expansion_reads_local_collections_without_any_remote_requests() {
        use httpmock::prelude::*;
        crate::http::install_crypto_provider();
        let server = MockServer::start();
        let remote = server.mock(|when, then| {
            when.method(GET);
            then.status(200)
                .body("[[sources]]\nurl = 'https://nested.example/feed'\n");
        });
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aggr.toml");
        std::fs::write(&root, "").unwrap();
        std::fs::write(dir.path().join("list.txt"), "./subscriptions.opml\n").unwrap();
        std::fs::write(dir.path().join("subscriptions.opml"), format!(r#"<opml><body><outline text="Local group"><outline text="News" xmlUrl="{}"/></outline></body></opml>"#, server.url("/feed"))).unwrap();
        let urls = [
            server.url("/config"),
            "https://github.com/o/r".to_string(),
            "./list.txt".to_string(),
        ];
        let sources = urls
            .iter()
            .map(|url| SourceConfig {
                url: Some(url.clone()),
                ..Default::default()
            })
            .collect();
        let expansion = expand(
            sources,
            root,
            &FetchConfig::default(),
            Some(Url::parse(&server.base_url()).unwrap()),
            false,
        )
        .await
        .unwrap();
        assert_eq!(expansion.sources.len(), 3);
        assert_eq!(expansion.sources[0].url.as_deref(), Some(urls[0].as_str()));
        assert_eq!(expansion.sources[1].url.as_deref(), Some(urls[1].as_str()));
        assert_eq!(
            expansion.sources[2].url.as_deref(),
            Some(server.url("/feed").as_str())
        );
        assert_eq!(expansion.sources[2].name.as_deref(), Some("News"));
        assert_eq!(
            expansion.sources[2].category.as_deref(),
            Some("Local group")
        );
        assert_eq!(expansion.local.len(), 3);
        assert!(expansion.remote.is_empty());
        remote.assert_calls(0);
    }

    #[tokio::test]
    async fn mixed_local_documents_flatten_in_order_with_metadata_defaults() {
        crate::http::install_crypto_provider();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aggr.toml");
        std::fs::write(&root, "").unwrap();
        std::fs::write(
            dir.path().join("news.opml"),
            r#"<opml><body><outline text="Local" xmlUrl="./news.json"/></body></opml>"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("news.json"),
            r#"{"version":"https://jsonfeed.org/version/1.1","title":"News","items":[]}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("nested.toml"), "[[sources]]\nurl = './news.opml'\nlabels = ['child']\nheaders = { authorization = 'child' }\nimages = false\n").unwrap();
        let source = SourceConfig {
            url: Some("./nested.toml".into()),
            category: Some("science".into()),
            labels: vec!["parent".into()],
            headers: BTreeMap::from([
                ("Authorization".into(), "parent".into()),
                ("Accept".into(), "application/xml".into()),
            ]),
            images: Some(true),
            ..Default::default()
        };
        let expansion = expand(vec![source], root, &FetchConfig::default(), None, true)
            .await
            .unwrap();
        assert_eq!(expansion.sources.len(), 1);
        let source = &expansion.sources[0];
        assert_eq!(source.name.as_deref(), Some("Local"));
        assert_eq!(source.category.as_deref(), Some("science"));
        assert_eq!(source.labels, ["parent", "child"]);
        assert_eq!(source.images, Some(false));
        assert_eq!(source.headers.len(), 2);
        assert_eq!(
            source.headers.get("authorization").map(String::as_str),
            Some("child")
        );
        assert_eq!(
            source.local_feed.as_deref(),
            Some(
                dir.path()
                    .join("news.json")
                    .canonicalize()
                    .unwrap()
                    .as_path()
            )
        );
        assert_eq!(expansion.local.len(), 4);
    }

    #[tokio::test]
    async fn remote_collection_detection_is_content_based_and_preserves_leaf_failures() {
        use httpmock::prelude::*;
        crate::http::install_crypto_provider();
        let server = MockServer::start();
        let collection = server.mock(|when, then| {
            when.method(GET)
                .path("/subscriptions")
                .header("x-test", "secret");
            then.status(200).body(format!(
                "[[sources]]\nurl = ['{}/feed', '{}/site', '{}/unavailable']\n",
                server.base_url(),
                server.base_url(),
                server.base_url()
            ));
        });
        server.mock(|when, then| {
            when.method(GET).path("/feed");
            then.status(200).body(
                r#"{"version":"https://jsonfeed.org/version/1.1","title":"News","items":[]}"#,
            );
        });
        server.mock(|when, then| {
            when.method(GET).path("/site");
            then.status(200).body("<html><body>News</body></html>");
        });
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aggr.toml");
        std::fs::write(&root, "").unwrap();
        let expansion = expand(
            vec![SourceConfig {
                url: Some(server.url("/subscriptions")),
                headers: BTreeMap::from([("x-test".into(), "secret".into())]),
                category: Some("group".into()),
                ..Default::default()
            }],
            root,
            &FetchConfig::default(),
            None,
            true,
        )
        .await
        .unwrap();
        collection.assert();
        assert_eq!(expansion.sources.len(), 3);
        assert_eq!(expansion.remote.len(), 1);
        for (source, path) in expansion
            .sources
            .iter()
            .zip(["/feed", "/site", "/unavailable"])
        {
            assert_eq!(source.url.as_deref(), Some(server.url(path).as_str()));
            assert_eq!(source.category.as_deref(), Some("group"));
        }
    }

    #[tokio::test]
    async fn relative_remote_feeds_resolve_and_different_headers_preserve_entries() {
        use httpmock::prelude::*;
        crate::http::install_crypto_provider();
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/collections/news");
            then.status(200).body("[[sources]]\nurl = './feed'\nheaders = { 'x-edition' = 'one' }\n[[sources]]\nurl = './feed'\nheaders = { 'x-edition' = 'two' }\n");
        });
        let feed = server.mock(|when, then| {
            when.method(GET).path("/collections/feed");
            then.status(200).body(
                r#"{"version":"https://jsonfeed.org/version/1.1","title":"News","items":[]}"#,
            );
        });
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aggr.toml");
        std::fs::write(&root, "").unwrap();
        let expansion = expand(
            vec![SourceConfig {
                url: Some(server.url("/collections/news")),
                ..Default::default()
            }],
            root,
            &FetchConfig::default(),
            None,
            true,
        )
        .await
        .unwrap();
        assert_eq!(expansion.sources.len(), 2);
        for (source, edition) in expansion.sources.iter().zip(["one", "two"]) {
            assert_eq!(
                source.url.as_deref(),
                Some(server.url("/collections/feed").as_str())
            );
            assert_eq!(
                source.headers.get("x-edition").map(String::as_str),
                Some(edition)
            );
        }
        feed.assert_calls(2);
    }

    #[tokio::test]
    async fn local_document_reads_obey_the_body_size_limit() {
        crate::http::install_crypto_provider();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("aggr.toml");
        std::fs::write(&root, "").unwrap();
        std::fs::write(
            dir.path().join("too-large.txt"),
            "https://example.org/feed\n",
        )
        .unwrap();
        let fetch = FetchConfig {
            max_body_bytes: 8,
            ..Default::default()
        };
        let expansion = expand(
            vec![SourceConfig {
                url: Some("./too-large.txt".into()),
                ..Default::default()
            }],
            root,
            &fetch,
            None,
            true,
        )
        .await
        .unwrap();
        assert!(expansion.sources.is_empty());
        assert_eq!(expansion.local.len(), 1);
    }

    #[test]
    fn github_repository_and_paths_are_inferred() {
        let repo = github_location(
            &Url::parse("https://github.com/aymericbeaumet/aggr-instance").unwrap(),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(repo.path, "aggr.toml");
        assert_eq!(repo.repo.owner, "aymericbeaumet");
        assert_eq!(repo.repo.name, "aggr-instance");
        assert_eq!(repo.repo.reference, None);

        let tree = github_location(
            &Url::parse("https://github.com/o/r/tree/main/topics").unwrap(),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(tree.path, "topics/aggr.toml");
        assert_eq!(tree.repo.reference.as_deref(), Some("main"));

        let wildcard = github_location(
            &Url::parse("https://github.com/o/r/blob/dev/topics/*.toml").unwrap(),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(wildcard.path, "topics/*.toml");
        assert_eq!(wildcard.repo.reference.as_deref(), Some("dev"));
    }

    #[test]
    fn repository_relative_paths_cannot_escape() {
        assert_eq!(
            repository_path("topics/aggr.toml", "./more/*.toml").unwrap(),
            "topics/more/*.toml"
        );
        assert_eq!(
            repository_path("topics/aggr.toml", "../root.toml").unwrap(),
            "root.toml"
        );
        assert!(repository_path("aggr.toml", "../outside.toml").is_err());
    }
}
