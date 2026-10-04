mod support;

use std::io::Cursor;
use std::path::PathBuf;

use assert_cmd::prelude::*;
use predicates::prelude::*;
use sha1::{Digest as _, Sha1};

struct Fixture {
    _temp: tempfile::TempDir,
    origin: PathBuf,
    clone: PathBuf,
    data: PathBuf,
    article: String,
    original: String,
    variant: String,
    initial: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let (origin, clone) = support::bare_origin_with_clone(temp.path()).unwrap();
        std::fs::write(clone.join("aggr.toml"), "[site]\ntitle = 'Storage'\n").unwrap();
        support::git(&clone, &["add", "aggr.toml"]).unwrap();
        support::git(&clone, &["commit", "-qm", "config"]).unwrap();
        support::git(&clone, &["push", "-q", "origin", "main"]).unwrap();
        std::fs::write(clone.join(".git/info/exclude"), "/.aggr/\n").unwrap();
        let data = clone.join(".aggr/data");
        std::fs::create_dir_all(&data).unwrap();
        support::git(&data, &["init", "-q", "-b", "aggr"]).unwrap();
        support::git(
            &data,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        )
        .unwrap();
        let directory = "items/demo/2026/10";
        std::fs::create_dir_all(data.join(directory)).unwrap();
        let mut original_bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(8, 8)
            .write_to(&mut original_bytes, image::ImageFormat::Png)
            .unwrap();
        let original_bytes = original_bytes.into_inner();
        let mut variant_bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 4)
            .write_to(&mut variant_bytes, image::ImageFormat::WebP)
            .unwrap();
        let variant_bytes = variant_bytes.into_inner();
        let original_name = format!(
            "article.image-{}.png",
            &hex::encode(Sha1::digest(&original_bytes))[..12]
        );
        let variant_name = format!(
            "article.image-{}.webp",
            &hex::encode(Sha1::digest(&variant_bytes))[..12]
        );
        let original = format!("{directory}/{original_name}");
        let variant = format!("{directory}/{variant_name}");
        let article = format!("{directory}/article.md");
        std::fs::write(data.join(&original), original_bytes).unwrap();
        std::fs::write(data.join(&variant), variant_bytes).unwrap();
        std::fs::write(data.join(&article), format!("---\ntitle: Hand edited\nlink: https://example.com/article\nsource: demo\nfirst_seen: 2026-10-01T10:00:00Z\npublished: 2026-09-30T10:00:00Z\ncustom: kept\nimages:\n  - source: https://example.com/picture.png\n    original:\n      file: {original_name}\n      width: 8\n      height: 8\n    variants:\n      - file: {variant_name}\n        width: 4\n        height: 4\n---\n\n\nHand-edited body.\r\n\nKeep spacing.\n")).unwrap();
        support::git(&data, &["add", "."]).unwrap();
        support::git(&data, &["commit", "-qm", "capture"]).unwrap();
        support::git(&data, &["push", "-q", "origin", "aggr"]).unwrap();
        let initial = support::git(&data, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_owned();
        Self {
            _temp: temp,
            origin,
            clone,
            data,
            article,
            original,
            variant,
            initial,
        }
    }

    fn run(&self) -> std::process::Command {
        support::aggr_command(&self.clone)
    }
}

#[test]
fn inspection_is_read_only_and_reports_separate_storage_categories() {
    let fixture = Fixture::new();
    let output = fixture
        .run()
        .args(["storage", "inspect", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(json["originals"]["bytes"].as_u64().unwrap() > 0);
    assert!(json["renditions"]["bytes"].as_u64().unwrap() > 0);
    assert!(
        json["current_tree"]["bytes"].as_u64().unwrap()
            > json["originals"]["bytes"].as_u64().unwrap()
    );
    assert!(json["git"].is_object());
    assert!(json["caches"]["deployment-media-v1"].is_object());
    assert!(
        support::git(&fixture.data, &["status", "--porcelain"])
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        support::git(&fixture.data, &["rev-parse", "HEAD"])
            .unwrap()
            .trim(),
        fixture.initial
    );
}

#[test]
fn prune_is_opt_in_preserves_original_body_and_history_and_is_idempotent() {
    let fixture = Fixture::new();
    let original = std::fs::read(fixture.data.join(&fixture.original)).unwrap();
    let before = std::fs::read_to_string(fixture.data.join(&fixture.article)).unwrap();
    fixture
        .run()
        .args(["storage", "prune-renditions", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Would prune 1"));
    assert!(fixture.data.join(&fixture.variant).exists());
    assert_eq!(
        std::fs::read_to_string(fixture.data.join(&fixture.article)).unwrap(),
        before
    );
    fixture
        .run()
        .args(["storage", "prune-renditions", "--apply"])
        .assert()
        .success()
        .stdout(predicate::str::contains("committed"));
    assert!(!fixture.data.join(&fixture.variant).exists());
    assert_eq!(
        std::fs::read(fixture.data.join(&fixture.original)).unwrap(),
        original
    );
    let after = std::fs::read_to_string(fixture.data.join(&fixture.article)).unwrap();
    assert_eq!(
        before.split_once("\n---\n").unwrap().1,
        after.split_once("\n---\n").unwrap().1
    );
    let before_yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(
        before
            .split_once("\n---\n")
            .unwrap()
            .0
            .trim_start_matches("---\n"),
    )
    .unwrap();
    let after_yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(
        after
            .split_once("\n---\n")
            .unwrap()
            .0
            .trim_start_matches("---\n"),
    )
    .unwrap();
    for name in ["title", "published", "first_seen", "custom"] {
        assert_eq!(before_yaml[name], after_yaml[name]);
    }
    support::git(
        &fixture.origin,
        &[
            "cat-file",
            "-e",
            &format!("{}:{}", fixture.initial, fixture.variant),
        ],
    )
    .unwrap();
    let head = support::git(&fixture.data, &["rev-parse", "HEAD"]).unwrap();
    assert_eq!(
        support::git(&fixture.origin, &["rev-parse", "refs/heads/aggr"]).unwrap(),
        head
    );
    fixture
        .run()
        .args(["storage", "prune-renditions", "--apply"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Pruning 0"));
    assert_eq!(
        support::git(&fixture.data, &["rev-parse", "HEAD"]).unwrap(),
        head
    );
}

#[test]
fn corrupt_master_keeps_its_rendition_and_dirty_archive_is_refused() {
    let fixture = Fixture::new();
    std::fs::write(fixture.data.join(&fixture.original), b"corrupt").unwrap();
    fixture
        .run()
        .args(["storage", "prune-renditions", "--apply"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("pending changes"));
    support::git(&fixture.data, &["add", "."]).unwrap();
    support::git(&fixture.data, &["commit", "-qm", "corrupt fixture"]).unwrap();
    fixture
        .run()
        .args(["storage", "prune-renditions", "--apply"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 images kept"));
    assert!(fixture.data.join(&fixture.variant).exists());
}
