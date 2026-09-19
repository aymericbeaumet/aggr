//! Local feed files next to `aggr.toml` are sources too; nothing here touches the network or the
//! user's git configuration.

mod support;

use assert_cmd::prelude::*;

use support::{aggr_command, bare_origin_with_clone, git};

#[test]
fn local_feed_sources_persist_articles_without_paths_and_repeat_without_a_commit() {
    let directory = tempfile::tempdir().unwrap();
    let (origin, repository) = bare_origin_with_clone(directory.path()).unwrap();
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
    git(&repository, &["add", "."]).unwrap();
    git(
        &repository,
        &["commit", "-q", "-m", "Configure saved feeds"],
    )
    .unwrap();
    git(&repository, &["push", "-q", "-u", "origin", "main"]).unwrap();
    aggr_command(&repository).arg("sync").assert().success();
    let first_tip = git(&origin, &["rev-parse", "aggr"]).unwrap();
    let files = git(&origin, &["ls-tree", "-r", "--name-only", "aggr"]).unwrap();
    let items: Vec<_> = files
        .lines()
        .filter(|path| path.starts_with("items/") && path.ends_with(".md"))
        .collect();
    assert_eq!(items.len(), 3);
    for path in files.lines() {
        let content = git(&origin, &["show", &format!("aggr:{path}")]).unwrap();
        assert!(!content.contains("file://"), "local endpoint in {path}");
        assert!(
            !content.contains(directory.path().to_str().unwrap()),
            "local path in {path}"
        );
        if items.contains(&path) {
            assert!(content.contains("https://example.com/"));
        }
    }
    aggr_command(&repository).arg("sync").assert().success();
    assert_eq!(git(&origin, &["rev-parse", "aggr"]).unwrap(), first_tip);
}

/// Every `index.html` under `root`, so pages are found by content rather than by slug.
fn html_pages(root: &std::path::Path) -> Vec<String> {
    let mut pages = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().is_some_and(|name| name == "index.html") {
                pages.push(std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    pages
}

/// A publisher's declared language is remembered per source and marks its articles with an
/// invisible `lang` attribute (WCAG 3.1.2, language of parts) only where it differs from the site
/// language; a source in the site language leaves the markup untouched.
#[test]
fn a_source_language_marks_its_articles_when_it_differs_from_the_site_language() {
    let directory = tempfile::tempdir().unwrap();
    let (origin, repository) = bare_origin_with_clone(directory.path()).unwrap();
    std::fs::write(
        repository.join("journal.xml"),
        r#"<rss version="2.0"><channel>
        <title>Journal</title><link>https://fr.example/</link><language>fr</language>
        <item><title>Article en français</title><link>/bonjour</link><guid>bonjour</guid>
        <description>Un texte en français.</description></item>
        </channel></rss>"#,
    )
    .unwrap();
    std::fs::write(
        repository.join("gazette.xml"),
        r#"<rss version="2.0"><channel>
        <title>Gazette</title><link>https://en.example/</link><language>en</language>
        <item><title>English article</title><link>/hello</link><guid>hello</guid>
        <description>An English text.</description></item>
        </channel></rss>"#,
    )
    .unwrap();
    std::fs::write(
        repository.join("aggr.toml"),
        r#"[site]
language = "en"

[site.params]
excerpts = true

[fetch]
content = "light"
images = false
previews = false

[[sources]]
url = "journal.xml"

[[sources]]
url = "gazette.xml"
"#,
    )
    .unwrap();
    git(&repository, &["add", "."]).unwrap();
    git(&repository, &["commit", "-q", "-m", "Configure two feeds"]).unwrap();
    git(&repository, &["push", "-q", "-u", "origin", "main"]).unwrap();
    aggr_command(&repository).arg("sync").assert().success();

    let files = git(&origin, &["ls-tree", "-r", "--name-only", "aggr"]).unwrap();
    let states: Vec<String> = files
        .lines()
        .filter(|path| path.starts_with("sources/") && path.ends_with("/state.toml"))
        .map(|path| git(&origin, &["show", &format!("aggr:{path}")]).unwrap())
        .collect();
    assert_eq!(states.len(), 2, "{files}");
    assert!(
        states
            .iter()
            .any(|state| state.contains("language = \"fr\"\n")),
        "the declared language is remembered with the source: {states:?}"
    );
    assert!(
        states
            .iter()
            .any(|state| state.contains("language = \"en\"\n")),
        "{states:?}"
    );

    aggr_command(&repository)
        .args(["build", "--out", "_site"])
        .assert()
        .success();
    let out = repository.join("_site");
    let pages = html_pages(&out.join("items"));
    let french = pages
        .iter()
        .find(|html| html.contains("<span class=\"itemhead-title\">Article en français</span>"))
        .expect("the French article page is built");
    assert!(french.contains("<html lang=\"en\">"), "{french}");
    assert!(
        french.contains("<article class=\"item h-entry\" lang=\"fr\" data-path=\""),
        "the article is marked as French: {french}"
    );
    let english = pages
        .iter()
        .find(|html| html.contains("<span class=\"itemhead-title\">English article</span>"))
        .expect("the English article page is built");
    assert!(
        english.contains("<article class=\"item h-entry\" data-path=\""),
        "a source in the site language adds no attribute: {english}"
    );
    assert_eq!(
        english.matches("lang=\"en\"").count(),
        1,
        "only the document declares the site language: {english}"
    );

    let river = std::fs::read_to_string(out.join("index.html")).unwrap();
    assert!(
        river.contains(" lang=\"fr\">Article en français</a>"),
        "{river}"
    );
    assert!(
        river.contains("<p class=\"excerpt\" lang=\"fr\">Un texte en français.</p>"),
        "{river}"
    );
    assert!(river.contains("\">English article</a>"), "{river}");
    assert!(
        river.contains("<p class=\"excerpt\">An English text.</p>"),
        "{river}"
    );
    assert_eq!(
        river.matches("lang=\"en\"").count(),
        1,
        "only the document declares the site language: {river}"
    );
}
