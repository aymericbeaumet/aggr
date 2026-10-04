mod support;

use assert_cmd::prelude::*;
use predicates::prelude::*;
use support::{aggr_command, git};

fn archive(body: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]).unwrap();
    std::fs::write(root.join("aggr.toml"), "[site]\nrepository = 'o/r'\n[[sources]]\nurl = 'https://example.com/feed'\nslug = 'demo'\nheaders = { Authorization = '${AGGR_OFFLINE_MISSING_SECRET}' }\n").unwrap();
    git(root, &["add", "."]).unwrap();
    git(root, &["commit", "-qm", "config"]).unwrap();
    git(root, &["checkout", "-q", "--orphan", "aggr"]).unwrap();
    git(root, &["rm", "-q", "-rf", "."]).unwrap();
    std::fs::create_dir_all(root.join("items/demo/2026/10")).unwrap();
    std::fs::write(root.join("items/demo/2026/10/article.md"), format!("---\ntitle: Local article\nlink: https://example.com/article\nsource: demo\npublished: 2026-10-01T12:00:00Z\nfirst_seen: 2026-10-01T12:00:00Z\n---\n\n{body}\n")).unwrap();
    git(root, &["add", "."]).unwrap();
    git(root, &["commit", "-qm", "archive"]).unwrap();
    git(root, &["checkout", "-q", "main"]).unwrap();
    git(
        root,
        &[
            "remote",
            "add",
            "origin",
            "https://127.0.0.1:1/unreachable.git",
        ],
    )
    .unwrap();
    tmp
}

#[test]
fn offline_build_does_not_contact_git_or_need_fetch_credentials() {
    let tmp = archive("An article stored entirely in Git.");
    let root = tmp.path();
    let refs = git(root, &["show-ref"]).unwrap();
    for args in [
        vec!["build", "--offline"],
        vec!["build", "--data-ref", "aggr"],
    ] {
        aggr_command(root)
            .env_remove("AGGR_OFFLINE_MISSING_SECRET")
            .env("GIT_TRACE", "1")
            .args(args)
            .assert()
            .success()
            .stderr(predicate::str::contains("built-in: git fetch").not());
        assert!(root.join("_site/index.html").exists());
        assert_eq!(git(root, &["show-ref"]).unwrap(), refs);
        assert!(!root.join(".git/FETCH_HEAD").exists());
        assert!(!root.join(".aggr/data").exists());
    }
}

#[test]
fn hermetic_build_rejects_unretained_images_before_publication() {
    let tmp = archive("Read this diagram: ![diagram](https://example.com/image.png)");
    aggr_command(tmp.path())
        .args(["build", "--hermetic"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("hermetic"));
    assert!(!tmp.path().join("_site/index.html").exists());
}

#[test]
fn hermetic_text_build_and_report_are_local() {
    let tmp = archive("A readable local article with an [original link](https://example.com).");
    aggr_command(tmp.path())
        .args(["build", "--hermetic", "--report", "report.json"])
        .assert()
        .success();
    let root = tmp.path();
    let page = std::fs::read_to_string(root.join("_site/items/demo/article/index.html")).unwrap();
    assert!(page.contains("Content-Security-Policy"));
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["hermetic"], true);
    assert_eq!(report["offline"], true);
    assert!(report["output_bytes"].as_u64().unwrap() > 0);
    assert!(!root.join(".git/FETCH_HEAD").exists());
}

#[test]
fn missing_local_ref_fails_without_fetching() {
    let tmp = archive("Local article");
    aggr_command(tmp.path())
        .args(["build", "--data-ref", "missing"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("locally"));
    assert!(!tmp.path().join(".git/FETCH_HEAD").exists());
}

#[test]
fn hermetic_article_keeps_video_as_an_original_link() {
    let tmp = archive("Watch the [recording](https://www.youtube.com/watch?v=abcDEF12345).");
    aggr_command(tmp.path())
        .args(["build", "--hermetic"])
        .assert()
        .success();
    let page =
        std::fs::read_to_string(tmp.path().join("_site/items/demo/article/index.html")).unwrap();
    let document = scraper::Html::parse_document(&page);
    let embeds = scraper::Selector::parse("iframe, video, audio, [data-video-embed]").unwrap();
    assert_eq!(document.select(&embeds).count(), 0);
    let links = scraper::Selector::parse("article a[href]").unwrap();
    assert!(document.select(&links).any(|link| {
        link.value()
            .attr("href")
            .is_some_and(|href| href.contains("abcDEF12345"))
    }));
}

#[test]
fn remote_policy_ignores_invalid_legacy_companions() {
    let tmp = archive("A remotely served ![diagram](https://example.com/image.png).");
    let root = tmp.path();
    git(root, &["checkout", "-q", "aggr"]).unwrap();
    let article = root.join("items/demo/2026/10/article.md");
    let original = std::fs::read_to_string(&article).unwrap();
    std::fs::write(&article, original.replacen("---\n", "---\npreview:\n  file: article.preview.png\n  width: 800\n  height: 600\nimages:\n  - source: https://example.com/image.png\n    original:\n      file: article.image-deadbeef0000.png\n      width: 800\n      height: 600\n", 1)).unwrap();
    for file in ["article.preview.png", "article.image-deadbeef0000.png"] {
        std::fs::write(
            root.join("items/demo/2026/10").join(file),
            "broken image bytes",
        )
        .unwrap();
    }
    git(root, &["add", "."]).unwrap();
    git(root, &["commit", "-qm", "legacy media"]).unwrap();
    git(root, &["checkout", "-q", "main"]).unwrap();
    aggr_command(root)
        .args(["build", "--offline"])
        .assert()
        .success();
    let page = std::fs::read_to_string(root.join("_site/items/demo/article/index.html")).unwrap();
    assert!(page.contains("https://example.com/image.png"));
    assert!(!page.contains("article.image-deadbeef0000.png"));
}

#[test]
fn failed_hermetic_build_preserves_previous_output() {
    let tmp = archive("A remotely served ![diagram](https://example.com/image.png).");
    let root = tmp.path();
    aggr_command(root)
        .args(["build", "--offline"])
        .assert()
        .success();
    let previous = std::fs::read(root.join("_site/index.html")).unwrap();
    aggr_command(root)
        .args(["build", "--hermetic"])
        .assert()
        .failure();
    assert_eq!(
        std::fs::read(root.join("_site/index.html")).unwrap(),
        previous
    );
}

#[test]
fn hermetic_build_uses_retained_images_and_a_pinned_clock() {
    use sha1::{Digest as _, Sha1};
    let tmp = archive("A preserved ![diagram](https://example.com/image.png).");
    let root = tmp.path();
    git(root, &["checkout", "-q", "aggr"]).unwrap();
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(16, 12)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let bytes = bytes.into_inner();
    let filename = format!(
        "article.image-{}.png",
        &hex::encode(Sha1::digest(&bytes))[..12]
    );
    let article = root.join("items/demo/2026/10/article.md");
    let original = std::fs::read_to_string(&article).unwrap();
    std::fs::write(&article, original.replacen("---\n", &format!("---\nimages:\n  - source: https://example.com/image.png\n    original:\n      file: {filename}\n      width: 16\n      height: 12\n"), 1)).unwrap();
    std::fs::write(root.join("items/demo/2026/10").join(filename), bytes).unwrap();
    git(root, &["add", "."]).unwrap();
    git(root, &["commit", "-qm", "preserved media"]).unwrap();
    git(root, &["checkout", "-q", "main"]).unwrap();
    let config = std::fs::read_to_string(root.join("aggr.toml")).unwrap();
    std::fs::write(
        root.join("aggr.toml"),
        format!("[defaults]\nmedia = 'local'\n{config}"),
    )
    .unwrap();
    let build = || {
        aggr_command(root)
            .env("SOURCE_DATE_EPOCH", "1790870400")
            .args(["build", "--hermetic"])
            .assert()
            .success();
        std::fs::read_to_string(root.join("_site/items/demo/article/index.html")).unwrap()
    };
    let page = build();
    let html = scraper::Html::parse_document(&page);
    let images = scraper::Selector::parse("article img").unwrap();
    let sources: Vec<_> = html
        .select(&images)
        .filter_map(|image| image.value().attr("src"))
        .collect();
    assert!(!sources.is_empty());
    assert!(sources.iter().all(|src| !src.starts_with("http")));
    assert_eq!(build(), page);
    assert!(!root.join(".git/FETCH_HEAD").exists());
}
