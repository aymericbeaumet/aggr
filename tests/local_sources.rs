use std::path::Path;
use std::process::Command;

use assert_cmd::prelude::*;

fn isolate(command: &mut Command) -> &mut Command {
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t");
    for name in [
        "GITHUB_ACTIONS",
        "GITHUB_REPOSITORY",
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "AGGR_BASE_URL",
        "AGGR_CONFIG",
        "AGGR_CACHE_DIR",
    ] {
        command.env_remove(name);
    }
    command
}

fn git(directory: &Path, arguments: &[&str]) -> String {
    let result = isolate(Command::new("git").current_dir(directory).args(arguments))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn local_feed_sources_persist_articles_without_paths_and_repeat_without_a_commit() {
    let directory = tempfile::tempdir().unwrap();
    git(
        directory.path(),
        &["init", "-q", "--bare", "-b", "main", "origin.git"],
    );
    git(directory.path(), &["clone", "-q", "origin.git", "repo"]);
    let repository = directory.path().join("repo");
    let origin = directory.path().join("origin.git");
    std::fs::write(repository.join("rss.xml"), r#"<rss version="2.0"><channel>
        <title>RSS</title><link>https://example.com/</link>
        <item><title>RSS article</title><link>/rss</link><guid>rss</guid><description>RSS body</description></item>
        </channel></rss>"#).unwrap();
    std::fs::write(
        repository.join("atom.xml"),
        r#"<feed xmlns="http://www.w3.org/2005/Atom">
        <title>Atom</title><link rel="self" href="https://example.com/feed.atom"/>
        <entry><id>atom</id><title>Atom article</title><link href="/atom"/>
        <updated>2026-09-01T10:00:00Z</updated><content type="text">Atom body</content></entry>
        </feed>"#,
    )
    .unwrap();
    std::fs::write(repository.join("feed.json"), r#"{"version":"https://jsonfeed.org/version/1.1",
        "title":"JSON","home_page_url":"https://example.com/","items":[
        {"id":"json","url":"https://example.com/json","title":"JSON article","content_text":"JSON body"}]}"#).unwrap();
    std::fs::write(
        repository.join("aggr.toml"),
        r#"[fetch]
content = "light"
images = false
previews = false

[[sources]]
url = ["rss.xml\natom.xml", "feed.json"]
category = "saved"
"#,
    )
    .unwrap();
    git(&repository, &["add", "."]);
    git(
        &repository,
        &["commit", "-q", "-m", "Configure saved feeds"],
    );
    git(&repository, &["push", "-q", "-u", "origin", "main"]);
    isolate(
        Command::cargo_bin("aggr")
            .unwrap()
            .current_dir(&repository)
            .arg("sync"),
    )
    .assert()
    .success();
    let first_tip = git(&origin, &["rev-parse", "aggr"]);
    let files = git(&origin, &["ls-tree", "-r", "--name-only", "aggr"]);
    let items: Vec<_> = files
        .lines()
        .filter(|path| path.starts_with("items/") && path.ends_with(".md"))
        .collect();
    assert_eq!(items.len(), 3);
    for path in files.lines() {
        let content = git(&origin, &["show", &format!("aggr:{path}")]);
        assert!(!content.contains("file://"), "local endpoint in {path}");
        assert!(
            !content.contains(directory.path().to_str().unwrap()),
            "local path in {path}"
        );
        if items.contains(&path) {
            assert!(content.contains("https://example.com/"));
        }
    }
    isolate(
        Command::cargo_bin("aggr")
            .unwrap()
            .current_dir(&repository)
            .arg("sync"),
    )
    .assert()
    .success();
    assert_eq!(git(&origin, &["rev-parse", "aggr"]), first_tip);
}
