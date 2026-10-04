//! Local render inputs captured by sync; never contains fetching credentials.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::Project;
use crate::config::{Config, SourceConfig};
use crate::discussions::ResolutionSet;
use crate::git::Repo;

const SOURCES: &str = ".aggr-sources.json";
const DISCUSSIONS: &str = ".aggr-discussions.json";

#[derive(Serialize, Deserialize)]
struct SourceSnapshot {
    version: u8,
    #[serde(default)]
    declarations: String,
    sources: Vec<SourceConfig>,
    remote: Vec<(String, String)>,
}

fn needs_snapshot(source: &SourceConfig) -> bool {
    source.collection
        || source.url.as_deref().is_some_and(|value| {
            value.contains("${")
                || url::Url::parse(value).is_ok_and(|url| {
                    let path = url.path().to_ascii_lowercase();
                    [".toml", ".opml", ".txt"]
                        .iter()
                        .any(|extension| path.ends_with(extension))
                })
        })
}

pub(super) async fn load(path: &Path, reference: Option<&str>) -> Result<Project> {
    let mut config = Config::load_offline(path).await?;
    let root = path
        .canonicalize()?
        .parent()
        .context("configuration parent")?
        .to_path_buf();
    let repo = Repo::discover(&root)?;
    let reference = reference
        .map(str::to_owned)
        .unwrap_or_else(|| format!("refs/heads/{}", config.store.branch));
    let sha = repo.local_commit(&reference)?;
    if config.sources.iter().any(needs_snapshot) {
        let bytes = repo.local_file(&sha, SOURCES)?.context(
            "offline source collection metadata is unavailable; sync first or use a fully local source configuration",
        )?;
        let snapshot: SourceSnapshot =
            serde_json::from_slice(&bytes).context("reading captured source metadata")?;
        apply_snapshot(&mut config, snapshot, &root)?;
    }
    for source in &mut config.sources {
        source.headers.clear();
    }
    let mut project = Project::from_config(path, config)?;
    project.local_data_sha = Some(sha);
    Ok(project)
}

fn declaration_fingerprint(sources: &[SourceConfig], root: &Path) -> Result<String> {
    let local_feeds: Vec<_> = sources
        .iter()
        .map(|source| {
            source
                .local_feed
                .as_deref()
                .map(|path| path.strip_prefix(root).unwrap_or(path))
        })
        .collect();
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&(
        sources,
        local_feeds,
    ))?)))
}

pub(super) async fn source_declarations(path: &Path) -> Result<String> {
    let config = Config::load_offline(path).await?;
    let canonical = path.canonicalize()?;
    let root = canonical.parent().context("configuration parent")?;
    declaration_fingerprint(&config.sources, root)
}

fn apply_snapshot(config: &mut Config, snapshot: SourceSnapshot, root: &Path) -> Result<()> {
    if snapshot.version != 3 {
        bail!("unsupported captured source metadata version; sync first");
    }
    if snapshot.declarations != declaration_fingerprint(&config.sources, root)? {
        bail!(
            "offline source declarations differ from the captured snapshot; sync first to capture the changed subscriptions or source options"
        );
    }
    // Matching declarations make captured public leaves authoritative. Keep local feed inputs,
    // which have no public URL and deliberately never enter the committed snapshot.
    let local: Vec<_> = std::mem::take(&mut config.sources)
        .into_iter()
        .filter(|source| source.local_feed.is_some())
        .collect();
    config.sources = snapshot.sources;
    config.sources.extend(local);
    config.loaded_remote = snapshot.remote;
    Ok(())
}

pub(super) async fn capture(
    project: &Project,
    data: &Path,
    discussions: Option<&ResolutionSet>,
) -> Result<()> {
    // Remote leaves cannot identify which current declaration produced them. Pin the locally
    // expanded declarations and reject changes until sync captures a new closure.
    let current = source_declarations(&project.config_path).await?;
    let declarations = match project.source_declarations.as_deref() {
        Some(initial) if initial != current => {
            bail!(
                "source declarations changed during sync; retry with the current configuration before capturing render metadata"
            );
        }
        Some(initial) => initial,
        None => &current,
    };
    let mut declarations_by_identity = std::collections::BTreeMap::new();
    for raw in &project.config.sources {
        let source = project.config.resolve_source(raw)?;
        declarations_by_identity
            .entry(source.identity)
            .or_insert(raw);
    }
    let mut sources = Vec::new();
    for source in &project.sources {
        let Some(public_url) = &source.public_url else {
            continue;
        };
        let raw = declarations_by_identity
            .get(&source.identity)
            .context("resolved source has no declaration")?;
        sources.push(SourceConfig {
            url: Some(public_url.clone()),
            slug: Some(source.slug.clone()),
            name: source.name.clone(),
            category: source.category.clone(),
            labels: source.labels.clone(),
            html: raw.html,
            content: raw.content,
            media: raw.media,
            limits: raw.limits,
            branch: raw.branch.clone(),
            sources: raw.sources.clone(),
            ..Default::default()
        });
    }
    write_changed(
        &data.join(SOURCES),
        &serde_json::to_vec(&SourceSnapshot {
            version: 3,
            declarations: declarations.to_owned(),
            sources,
            remote: project
                .config
                .loaded_remote
                .iter()
                .map(|(url, digest)| {
                    let sanitized = url::Url::parse(url)
                        .map(|mut url| {
                            let _ = url.set_username("");
                            let _ = url.set_password(None);
                            url.set_query(None);
                            url.set_fragment(None);
                            url.to_string()
                        })
                        .unwrap_or_else(|_| url.clone());
                    (sanitized, digest.clone())
                })
                .collect(),
        })?,
    )?;
    if let Some(discussions) = discussions {
        write_changed(&data.join(DISCUSSIONS), &serde_json::to_vec(discussions)?)?;
    }
    Ok(())
}

fn write_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    match std::fs::read(path) {
        Ok(existing) if existing == bytes => return Ok(()),
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
        _ => {}
    }
    std::fs::write(path, bytes).with_context(|| format!("saving {}", path.display()))
}

pub(super) fn discussions(data: &Path) -> Result<ResolutionSet> {
    match std::fs::read(data.join(DISCUSSIONS)) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("reading captured discussion metadata"),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(ResolutionSet::default()),
        Err(err) => Err(err.into()),
    }
}

pub(super) fn clock(repo: &Repo, sha: Option<&str>) -> Result<chrono::DateTime<chrono::Utc>> {
    if let Some(value) = std::env::var_os("SOURCE_DATE_EPOCH") {
        let seconds = value
            .to_str()
            .context("SOURCE_DATE_EPOCH must be UTF-8")?
            .parse::<i64>()
            .context("SOURCE_DATE_EPOCH must be an integer Unix timestamp")?;
        if seconds < 0 {
            bail!("SOURCE_DATE_EPOCH must be nonnegative");
        }
        return chrono::DateTime::from_timestamp(seconds, 0)
            .context("SOURCE_DATE_EPOCH is out of range");
    }
    sha.map_or_else(|| Ok(chrono::Utc::now()), |sha| repo.local_commit_time(sha))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declarations() -> Config {
        Config::parse("[[sources]]\nurl = 'https://example.com/list.opml'\n[[sources]]\nurl = 'https://example.com/feed'\nslug = 'ordinary'\n").unwrap()
    }

    fn snapshot(config: &Config) -> SourceSnapshot {
        SourceSnapshot {
            version: 3,
            declarations: declaration_fingerprint(&config.sources, Path::new("/project")).unwrap(),
            sources: vec![
                SourceConfig {
                    url: Some("https://collection.example/feed".into()),
                    slug: Some("collected".into()),
                    ..Default::default()
                },
                config.sources[1].clone(),
            ],
            remote: vec![("https://example.com/list.opml".into(), "content-sha".into())],
        }
    }

    #[test]
    fn changed_collection_declaration_requires_a_new_snapshot() {
        for change in ["url", "images", "remove_ordinary", "add_collection"] {
            let mut config = declarations();
            let saved = snapshot(&config);
            match change {
                "url" => config.sources[0].url = Some("https://other.example/list.opml".into()),
                "images" => config.sources[0].media = Some(crate::config::MediaPolicy::Local),
                "remove_ordinary" => {
                    config.sources.pop();
                }
                "add_collection" => config.sources.push(SourceConfig {
                    url: Some("https://other.example/list.opml".into()),
                    ..Default::default()
                }),
                _ => unreachable!(),
            }
            assert!(
                apply_snapshot(&mut config, saved, Path::new("/project")).is_err(),
                "{change}"
            );
        }
    }

    #[test]
    fn unchanged_declarations_keep_current_global_media_defaults() {
        let mut config = declarations();
        let saved = snapshot(&config);
        config.defaults.media = crate::config::MediaPolicy::Local;
        apply_snapshot(&mut config, saved, Path::new("/project")).unwrap();
        assert_eq!(config.sources.len(), 2);
        let sources = config.resolve_sources(&|_| None).unwrap();
        assert!(
            sources
                .iter()
                .all(|source| source.images == crate::config::ImagePolicy::Original)
        );
        assert_eq!(config.loaded_remote.len(), 1);
    }

    #[test]
    fn ordinary_source_with_a_sanitized_url_is_not_duplicated() {
        let mut config = declarations();
        config.sources[1].slug = None;
        config.sources[1].url = Some("https://user:password@example.com/feed?token=secret".into());
        let mut saved = snapshot(&config);
        let ordinary = config.resolve_source(&config.sources[1]).unwrap();
        saved.sources[1].url = ordinary.public_url;
        saved.sources[1].slug = Some(ordinary.slug);
        apply_snapshot(&mut config, saved, Path::new("/project")).unwrap();
        assert_eq!(config.sources.len(), 2);
    }

    #[test]
    fn local_feed_paths_participate_in_declarations_without_binding_checkout_location() {
        let mut sources = vec![SourceConfig {
            local_feed: Some("/first/feeds/local.xml".into()),
            ..Default::default()
        }];
        let first = declaration_fingerprint(&sources, Path::new("/first")).unwrap();
        sources[0].local_feed = Some("/second/feeds/local.xml".into());
        assert_eq!(
            first,
            declaration_fingerprint(&sources, Path::new("/second")).unwrap()
        );
        sources[0].local_feed = Some("/second/feeds/other.xml".into());
        assert_ne!(
            first,
            declaration_fingerprint(&sources, Path::new("/second")).unwrap()
        );
    }

    #[tokio::test]
    async fn capture_pins_unexpanded_declarations_without_persisting_credentials() {
        crate::http::install_crypto_provider();
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        let path = tmp.path().join("aggr.toml");
        std::fs::write(&path, "[[sources]]\nurl = 'https://example.com/list.opml'\nheaders = { Authorization = 'declaration-secret' }\n").unwrap();
        let declarations = Config::load_offline(&path).await.unwrap();
        let mut expanded = Config::parse("[[sources]]\nurl = 'https://user:password@collection.example/feed?token=expanded-secret'\nheaders = { Authorization = 'header-secret' }\n[[sources]]\nurl = 'https://collection.example/other-feed'\n").unwrap();
        expanded.loaded_files = declarations.loaded_files.clone();
        let project = Project::from_config(&path, expanded).unwrap();
        capture(&project, tmp.path(), None).await.unwrap();
        let bytes = std::fs::read(tmp.path().join(SOURCES)).unwrap();
        let json = String::from_utf8(bytes.clone()).unwrap();
        for secret in [
            "declaration-secret",
            "expanded-secret",
            "header-secret",
            "password",
        ] {
            assert!(!json.contains(secret), "snapshot exposed {secret}");
        }
        let captured: SourceSnapshot = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            captured
                .sources
                .iter()
                .map(|source| source.slug.as_deref().unwrap())
                .collect::<Vec<_>>(),
            project
                .sources
                .iter()
                .map(|source| source.slug.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            captured.declarations,
            declaration_fingerprint(&declarations.sources, &project.root).unwrap()
        );
    }

    #[tokio::test]
    async fn changing_declarations_during_sync_preserves_the_previous_snapshot() {
        crate::http::install_crypto_provider();
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(tmp.path())
                .status()
                .unwrap()
                .success()
        );
        let path = tmp.path().join("aggr.toml");
        std::fs::write(
            &path,
            "[[sources]]\nurl = 'https://example.com/original/feed'\n",
        )
        .unwrap();
        let project = Project::load(&path).await.unwrap();
        capture(&project, tmp.path(), None).await.unwrap();
        let original = std::fs::read(tmp.path().join(SOURCES)).unwrap();
        std::fs::write(
            &path,
            "[[sources]]\nurl = 'https://example.com/replacement/feed'\n",
        )
        .unwrap();
        let error = capture(&project, tmp.path(), None).await.unwrap_err();
        assert!(error.to_string().contains("changed during sync"));
        assert_eq!(std::fs::read(tmp.path().join(SOURCES)).unwrap(), original);
    }
}
