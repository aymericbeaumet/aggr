//! Safe, deterministic expansion of local and remote aggr configuration graphs.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;
use url::Url;

use super::{FetchConfig, SourceConfig, validate_source_file_entry};
use crate::http::{self, Request, Response};

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
) -> Result<Expansion> {
    let client = http::Client::new(fetch)?;
    let root = root
        .canonicalize()
        .with_context(|| format!("resolving {}", root.display()))?;
    let root_location = Location::Local(root.clone());
    let mut loader = Loader {
        client,
        allow_remote_chains: fetch.allow_remote_include_chains,
        github_api,
        seen: BTreeSet::from([root_location.key()]),
        local: vec![root],
        remote: Vec::new(),
        default_branches: BTreeMap::new(),
        trees: BTreeMap::new(),
    };
    let sources = loader.expand_sources(sources, root_location, None).await;
    Ok(Expansion {
        sources,
        local: loader.local,
        remote: loader.remote,
    })
}

struct Loader {
    client: http::Client,
    allow_remote_chains: bool,
    github_api: Option<Url>,
    seen: BTreeSet<String>,
    local: Vec<PathBuf>,
    remote: Vec<(String, String)>,
    default_branches: BTreeMap<String, String>,
    trees: BTreeMap<String, Vec<String>>,
}

impl Loader {
    fn expand_sources(
        &mut self,
        sources: Vec<SourceConfig>,
        declaring: Location,
        inherited_category: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Vec<SourceConfig>> + Send + '_>> {
        Box::pin(async move {
            let mut expanded = Vec::new();
            for (index, mut source) in sources.into_iter().enumerate() {
                let Some(pattern) = source.include.clone() else {
                    if source.category.as_deref().is_none_or(str::is_empty) {
                        source.category = inherited_category.clone();
                    }
                    expanded.push(source);
                    continue;
                };
                if let Err(err) = validate_source_file_entry(&source) {
                    warn(
                        &format!("[[sources]] #{} in {}", index + 1, declaring),
                        &err,
                    );
                    continue;
                }
                let category = source
                    .category
                    .as_deref()
                    .filter(|category| !category.is_empty())
                    .map(str::to_string)
                    .or_else(|| inherited_category.clone());
                let targets = match self.targets(&declaring, &pattern).await {
                    Ok(targets) => targets,
                    Err(err) => {
                        warn(
                            &format!("include {} from {declaring}", include_label(&pattern)),
                            &err,
                        );
                        continue;
                    }
                };
                for target in targets {
                    let key = target.key();
                    if !self.seen.insert(key) {
                        log::warn!("ignoring repeated or cyclic include {target}");
                        continue;
                    }
                    let (included, actual) = match self.read(target).await {
                        Ok(value) => value,
                        Err(err) => {
                            warn(
                                &format!("include {} from {declaring}", include_label(&pattern)),
                                &err,
                            );
                            continue;
                        }
                    };
                    let nested = self
                        .expand_sources(included, actual, category.clone())
                        .await;
                    expanded.extend(nested);
                }
            }
            expanded
        })
    }

    async fn targets(&mut self, declaring: &Location, pattern: &str) -> Result<Vec<Location>> {
        if pattern.trim().is_empty() {
            bail!("`include` must name an aggr config or pattern");
        }
        if let Some(url) = remote_url(pattern)? {
            if declaring.is_remote() && !self.allow_remote_chains {
                bail!(
                    "a remote config cannot include another remote URL unless [fetch] \
                     allow_remote_include_chains = true"
                );
            }
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
                    bail!("a remote config may only include a relative path on its own host");
                }
                let joined = url.join(pattern).with_context(|| {
                    format!(
                        "resolving {} against {}",
                        include_label(pattern),
                        safe_url(url)
                    )
                })?;
                if joined.origin() != url.origin() {
                    bail!("a remote config may only include a relative path on its own host");
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
            .with_context(|| format!("invalid remote include glob {path:?}"))?;
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
            bail!("remote include glob {path:?} matched no file in {repo}");
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
            bail!("GitHub truncated the tree for {repo}@{reference}; narrow the include pattern");
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

    async fn read(&mut self, location: Location) -> Result<(Vec<SourceConfig>, Location)> {
        match location {
            Location::Local(path) => {
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                let sources = included_sources(&text)
                    .with_context(|| format!("{} is not an aggr config", path.display()))?;
                self.local.push(path.clone());
                Ok((sources, Location::Local(path)))
            }
            Location::Generic(url) => {
                let body = self.get(&url, &[]).await?;
                let actual = Location::Generic(body.final_url.clone());
                self.seen.insert(actual.key());
                let text = std::str::from_utf8(&body.bytes)
                    .with_context(|| format!("{} is not UTF-8 TOML", safe_url(&url)))?;
                let sources = included_sources(text)
                    .with_context(|| format!("{} is not an aggr config", safe_url(&url)))?;
                self.record_remote(&actual, &body.bytes);
                Ok((sources, actual))
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
                let text = std::str::from_utf8(&bytes)
                    .with_context(|| format!("{file} is not UTF-8 TOML"))?;
                let sources = included_sources(text)
                    .with_context(|| format!("{file} is not an aggr config"))?;
                let location = Location::GitHub(file);
                self.record_remote(&location, &bytes);
                Ok((sources, location))
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

    fn record_remote(&mut self, location: &Location, bytes: &[u8]) {
        self.remote
            .push((location.key(), crate::model::sha1_hex(bytes)));
    }
}

#[derive(Clone, Debug)]
enum Location {
    Local(PathBuf),
    Generic(Url),
    GitHub(GitHubFile),
}

impl Location {
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
            bail!("remote include {value:?} must use http or https");
        }
        return Ok(None);
    }
    let Ok(url) = Url::parse(value) else {
        bail!("remote include {value:?} is not a valid URL");
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
                "GitHub include {} must identify owner/repository",
                safe_url(url)
            )
        }
        _ => return Ok(None),
    };
    if owner.is_empty() || name.is_empty() {
        bail!(
            "GitHub include {} must identify owner/repository",
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
                    bail!("remote include {path:?} escapes its repository");
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                bail!("remote include {path:?} is not a repository-relative path")
            }
        }
    }
    if components.is_empty() {
        bail!("remote include path is empty");
    }
    Ok(components.join("/"))
}

fn local_targets(dir: &Path, pattern: &str) -> Result<Vec<Location>> {
    let full = dir.join(pattern);
    let mut paths = if has_glob(pattern) {
        let pattern = full.to_string_lossy();
        glob::glob(&pattern)
            .context("invalid local include glob")?
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
            "include glob {pattern:?} matched no file under {}",
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

fn include_label(value: &str) -> String {
    remote_url(value)
        .ok()
        .flatten()
        .map_or_else(|| format!("{value:?}"), |url| safe_url(&url))
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct IncludedConfig {
    sources: Vec<SourceConfig>,
}

fn included_sources(text: &str) -> Result<Vec<SourceConfig>> {
    let included: IncludedConfig = toml::from_str(text)?;
    if included.sources.is_empty() {
        bail!("it contains no [[sources]] entries");
    }
    Ok(included.sources)
}

fn warn(context: &str, error: &anyhow::Error) {
    log::warn!("ignoring {context}: {error:#}");
}

#[cfg(test)]
mod tests {
    use super::*;

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
