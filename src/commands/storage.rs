//! Archive accounting and an explicit, append-only migration of optional image renditions.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail, ensure};
use serde::Serialize;
use serde_yaml_ng::Value;

use super::Project;
use crate::cli::{StorageArgs, StorageCommand};
use crate::git::{CommitMessage, PushOutcome, Worktree};
use crate::model::ArticleImage;

pub fn run(project: &Project, args: &StorageArgs) -> Result<()> {
    match &args.command {
        StorageCommand::Inspect { json } => inspect(project, *json),
        StorageCommand::PruneRenditions { apply, .. } => prune(project, *apply),
    }
}

fn data_dir(project: &Project) -> PathBuf {
    project.repo.root().join(&project.config.store.dir)
}

#[derive(Debug, Default, Serialize)]
struct TreeBytes {
    files: usize,
    bytes: u64,
}

fn tree_bytes(root: &Path) -> Result<TreeBytes> {
    let mut total = TreeBytes::default();
    let metadata = match std::fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(total),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "{} is not an ordinary directory",
        root.display()
    );
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != ".git")
    {
        let entry = entry?;
        ensure!(
            !entry.file_type().is_symlink(),
            "refusing redirected storage path {}",
            entry.path().display()
        );
        if entry.file_type().is_file() {
            total.files += 1;
            total.bytes = total.bytes.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(total)
}

#[derive(Debug, Default, Serialize)]
struct GitBytes {
    loose_objects: u64,
    packed_objects: u64,
    loose_bytes: u64,
    packed_bytes: u64,
    garbage_bytes: u64,
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    ensure!(
        output.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).context("reading git output")
}

fn object_bytes(root: &Path) -> Result<GitBytes> {
    let output = git(root, &["count-objects", "-v"])?;
    let values: BTreeMap<_, _> = output
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(": ")?;
            Some((key, value.parse::<u64>().ok()?))
        })
        .collect();
    let get = |key| values.get(key).copied().unwrap_or_default();
    Ok(GitBytes {
        loose_objects: get("count"),
        packed_objects: get("in-pack"),
        loose_bytes: get("size") * 1024,
        packed_bytes: get("size-pack") * 1024,
        garbage_bytes: get("size-garbage") * 1024,
    })
}

#[derive(Serialize)]
struct Inspection {
    current_tree: TreeBytes,
    originals: TreeBytes,
    renditions: TreeBytes,
    output: TreeBytes,
    caches: BTreeMap<String, TreeBytes>,
    git: GitBytes,
    history: &'static str,
}

fn inspect(project: &Project, json: bool) -> Result<()> {
    let data = data_dir(project);
    let mut originals = BTreeSet::new();
    let mut renditions = BTreeSet::new();
    if data.is_dir() {
        for item in crate::store::Store::open(&data).items()? {
            let relative = Path::new(&item.path);
            let Some(directory) = relative.parent() else {
                continue;
            };
            let Some(stem) = relative.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            for image in item
                .front
                .images
                .iter()
                .filter(|image| image.is_valid_for(stem))
            {
                originals.insert(directory.join(&image.original.file));
                renditions.extend(
                    image
                        .variants
                        .iter()
                        .map(|image| directory.join(&image.file)),
                );
            }
        }
    }
    let sum = |paths: &BTreeSet<PathBuf>| -> Result<TreeBytes> {
        let mut sum = TreeBytes::default();
        for relative in paths {
            match safe_file(&data, relative) {
                Ok(path) => {
                    sum.files += 1;
                    sum.bytes += path.metadata()?.len();
                }
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(sum)
    };
    let cache = crate::cache::build(project.repo.root());
    let caches = crate::cache::Namespace::ALL
        .iter()
        .map(|namespace| {
            Ok((
                namespace.dir_name().to_owned(),
                tree_bytes(&namespace.dir(&cache))?,
            ))
        })
        .collect::<Result<_>>()?;
    let report = Inspection {
        current_tree: tree_bytes(&data)?,
        originals: sum(&originals)?,
        renditions: sum(&renditions)?,
        output: tree_bytes(&project.root.join(&project.config.site.out))?,
        caches,
        git: object_bytes(project.repo.root())?,
        history: "Deleting current-tree renditions does not reclaim append-only Git history or invalidate old blob URLs.",
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Current archive: {} bytes in {} files",
            report.current_tree.bytes, report.current_tree.files
        );
        println!(
            "Original images: {} bytes; optional renditions: {} bytes",
            report.originals.bytes, report.renditions.bytes
        );
        println!("Published output: {} bytes", report.output.bytes);
        println!(
            "Git objects: {} packed bytes, {} loose bytes",
            report.git.packed_bytes, report.git.loose_bytes
        );
        for (name, usage) in report.caches {
            println!("Cache {name}: {} bytes", usage.bytes);
        }
        println!("{}", report.history);
    }
    Ok(())
}

fn safe_file(root: &Path, relative: &Path) -> Result<PathBuf> {
    ensure!(
        std::fs::symlink_metadata(root)?.is_dir(),
        "archive root is not an ordinary directory"
    );
    let mut path = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            bail!("invalid archive path {}", relative.display());
        };
        path.push(name);
        ensure!(
            !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "refusing redirected archive path {}",
            path.display()
        );
    }
    ensure!(
        std::fs::symlink_metadata(&path)?.is_file(),
        "{} is not a regular file",
        path.display()
    );
    Ok(path)
}

struct Edit {
    path: PathBuf,
    before: Vec<u8>,
    after: Vec<u8>,
}
#[derive(Default)]
struct PrunePlan {
    edits: Vec<Edit>,
    removals: BTreeMap<PathBuf, u64>,
    originals: BTreeMap<PathBuf, String>,
    skipped_images: usize,
}

fn plan_prune(root: &Path) -> Result<PrunePlan> {
    let mut plan = PrunePlan::default();
    let store = crate::store::Store::open(root);
    for item_path in store.item_paths()? {
        let relative = PathBuf::from(format!("{item_path}.md"));
        let path = safe_file(root, &relative)?;
        let before = std::fs::read(&path)?;
        let text = std::str::from_utf8(&before).context("article Markdown is not UTF-8")?;
        let (yaml, _) = crate::store::frontmatter::split(text)?;
        let mut document: Value = serde_yaml_ng::from_str(yaml)?;
        let Some(images) = document.get_mut("images").and_then(Value::as_sequence_mut) else {
            continue;
        };
        let protected: BTreeSet<String> = images
            .iter()
            .filter_map(|image| {
                image
                    .get("original")?
                    .get("file")?
                    .as_str()
                    .map(str::to_owned)
            })
            .collect();
        let directory = relative
            .parent()
            .context("article has a parent directory")?;
        let stem = relative
            .file_stem()
            .and_then(|stem| stem.to_str())
            .context("article stem is UTF-8")?;
        let mut changed = false;
        for image in images {
            let metadata = match serde_yaml_ng::from_value::<ArticleImage>(image.clone()) {
                Ok(metadata) if metadata.is_valid_for(stem) && !metadata.variants.is_empty() => {
                    metadata
                }
                _ => continue,
            };
            let original = match safe_file(root, &directory.join(&metadata.original.file)) {
                Ok(original) => original,
                Err(_) => {
                    plan.skipped_images += 1;
                    continue;
                }
            };
            if original.metadata()?.len()
                > crate::media::MediaLimits::default().max_file_bytes as u64
            {
                plan.skipped_images += 1;
                continue;
            }
            let bytes = std::fs::read(&original)?;
            if crate::media::validate_stored(
                &bytes,
                &metadata.original,
                crate::media::StoredKind::Master,
            )
            .is_err()
            {
                plan.skipped_images += 1;
                continue;
            }
            plan.originals
                .insert(original, crate::model::sha1_hex(&bytes));
            for variant in &metadata.variants {
                if protected.contains(&variant.file) {
                    continue;
                }
                let relative = directory.join(&variant.file);
                match safe_file(root, &relative) {
                    Ok(path) => {
                        plan.removals.insert(path.clone(), path.metadata()?.len());
                    }
                    Err(error)
                        if error
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
                    Err(error) => return Err(error),
                }
            }
            image
                .as_mapping_mut()
                .context("image metadata is a mapping")?
                .remove(Value::String("variants".into()));
            changed = true;
        }
        if changed {
            // Keep the closing fence and every body byte, including blank lines and CRLFs.
            let start = if text.starts_with("---\r\n") { 5 } else { 4 };
            let after = format!(
                "{}{}{}",
                &text[..start],
                serde_yaml_ng::to_string(&document)?,
                &text[start + yaml.len()..]
            )
            .into_bytes();
            plan.edits.push(Edit {
                path,
                before,
                after,
            });
        }
    }
    Ok(plan)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut temporary =
        tempfile::NamedTempFile::new_in(path.parent().context("article has a parent")?)?;
    temporary.write_all(bytes)?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn prune(project: &Project, apply: bool) -> Result<()> {
    let _guard = apply.then(|| project.lock("storage")).transpose()?;
    let root = data_dir(project);
    let worktree = Worktree::open_existing(&root, &project.config.store.branch)?;
    let plan = plan_prune(&root)?;
    let bytes: u64 = plan.removals.values().sum();
    println!(
        "{}{} rendition files ({} current-tree bytes) in {} articles; {} images kept because their original could not be verified",
        if apply { "Pruning " } else { "Would prune " },
        plan.removals.len(),
        bytes,
        plan.edits.len(),
        plan.skipped_images
    );
    println!(
        "Original images and article bodies remain unchanged. Existing Git history and old blob URLs remain available; historical Git bytes are not reclaimed."
    );
    if !apply || plan.edits.is_empty() {
        return Ok(());
    }
    for edit in &plan.edits {
        ensure!(
            std::fs::read(&edit.path)? == edit.before,
            "article changed during inspection: {}",
            edit.path.display()
        );
    }
    for (path, hash) in &plan.originals {
        ensure!(
            crate::model::sha1_hex(std::fs::read(path)?) == *hash,
            "original changed during inspection: {}",
            path.display()
        );
    }
    let backup = tempfile::Builder::new()
        .prefix("prune-renditions-")
        .tempdir_in(root.parent().context("archive has a parent")?)?;
    let mut moved = Vec::new();
    let result = (|| -> Result<Option<String>> {
        for (index, path) in plan.removals.keys().enumerate() {
            let destination = backup.path().join(index.to_string());
            std::fs::rename(path, &destination)?;
            moved.push((path, destination));
        }
        for edit in &plan.edits {
            write_atomic(&edit.path, &edit.after)?;
        }
        worktree.commit(&CommitMessage {
            subject: "chore: prune optional archived image renditions".into(),
            ..CommitMessage::default()
        })
    })();
    let sha = match result {
        Ok(sha) => sha,
        Err(error) => {
            for edit in &plan.edits {
                write_atomic(&edit.path, &edit.before)?;
            }
            for (path, backup) in moved {
                std::fs::rename(backup, path)?;
            }
            let paths: Vec<_> = plan
                .edits
                .iter()
                .map(|edit| edit.path.as_path())
                .chain(plan.removals.keys().map(PathBuf::as_path))
                .collect();
            for chunk in paths.chunks(64) {
                let relative = chunk
                    .iter()
                    .map(|path| {
                        path.strip_prefix(&root)
                            .map(|path| path.to_string_lossy().into_owned())
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let mut args = vec!["reset", "-q", "HEAD", "--"];
                args.extend(relative.iter().map(String::as_str));
                git(&root, &args)?;
            }
            return Err(error).context("pruning failed; restored the archive");
        }
    };
    if let Some(sha) = sha {
        println!("committed {sha}");
        if worktree.push().context(
            "renditions were pruned in a local commit; publication failed, run sync to retry",
        )? == PushOutcome::Pushed
        {
            println!("pushed {}", worktree.branch());
        }
    }
    Ok(())
}
