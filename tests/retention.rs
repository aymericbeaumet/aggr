mod support;

use assert_cmd::prelude::*;
use chrono::{Duration, Utc};
use httpmock::MockServer;
use std::path::{Path, PathBuf};
use support::{aggr_command, git};

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    server: MockServer,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let (_, root) = support::bare_origin_with_clone(tmp.path()).unwrap();
        let server = MockServer::start();
        for source in ["alpha", "beta"] {
            let entries = (0..3).map(|i| format!("<item><title>{source}-{i}</title><guid>{source}-{i}</guid><link>https://example.com/{source}/{i}</link><pubDate>{}</pubDate><description>{source} article {i} has its own body.</description></item>", (Utc::now() - Duration::days(i)).to_rfc2822())).collect::<String>();
            let path = format!("/{source}.xml");
            server.mock(|when, then| {
                when.path(path);
                then.status(200).header("content-type", "application/rss+xml").body(format!("<rss version='2.0'><channel><title>{source}</title><link>https://example.com/{source}</link><description>Articles</description>{entries}</channel></rss>"));
            });
        }
        let fixture = Self {
            _tmp: tmp,
            root,
            server,
        };
        fixture.configure("max_items = 0\nmax_age_days = 0\nmax_bytes = 0", "", "");
        git(&fixture.root, &["add", "aggr.toml"]).unwrap();
        git(&fixture.root, &["commit", "-qm", "config"]).unwrap();
        git(&fixture.root, &["push", "-q", "origin", "main"]).unwrap();
        fixture.sync();
        fixture
    }

    fn configure(&self, defaults: &str, alpha: &str, beta: &str) {
        std::fs::write(self.root.join("aggr.toml"), format!("[site]\ntitle = 'Limits'\n[defaults]\n{defaults}\n[[sources]]\nurl = '{}'\nslug = 'alpha'\n{alpha}\n[[sources]]\nurl = '{}'\nslug = 'beta'\n{beta}\n", self.server.url("/alpha.xml"), self.server.url("/beta.xml"))).unwrap();
    }

    fn data(&self) -> PathBuf {
        self.root.join(".aggr/data")
    }
    fn sync(&self) {
        aggr_command(&self.root).arg("sync").assert().success();
    }
    fn articles(&self, source: &str) -> Vec<PathBuf> {
        walkdir::WalkDir::new(self.data().join("items").join(source))
            .into_iter()
            .map(Result::unwrap)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "md")
            })
            .map(|entry| entry.into_path())
            .collect()
    }
    fn article(&self, title: &str) -> PathBuf {
        ["alpha", "beta"]
            .into_iter()
            .flat_map(|source| self.articles(source))
            .find(|path| {
                std::fs::read_to_string(path)
                    .unwrap()
                    .contains(&format!("title: {title}"))
            })
            .unwrap()
    }
    fn head(&self) -> String {
        git(&self.data(), &["rev-parse", "HEAD"]).unwrap()
    }
    fn save(&self) {
        git(&self.data(), &["add", "."]).unwrap();
        git(&self.data(), &["commit", "-qm", "extra companions"]).unwrap();
        git(&self.data(), &["push", "-q", "origin", "aggr"]).unwrap();
    }
}

fn companion(article: &Path, suffix: &str) -> PathBuf {
    article.with_file_name(format!(
        "{}{suffix}",
        article.file_stem().unwrap().to_str().unwrap()
    ))
}

#[test]
fn count_limits_remove_whole_families_only_from_the_affected_feed() {
    let fixture = Fixture::new();
    let old = fixture.article("alpha-2");
    let companions: Vec<_> = [
        ".html",
        ".image-123456789abc.png",
        ".preview-123456789abc.jpg",
        ".document-123456789abc.pdf",
    ]
    .into_iter()
    .map(|suffix| companion(&old, suffix))
    .collect();
    for file in &companions {
        std::fs::write(file, "obsolete companion").unwrap();
    }
    let unrelated = old.with_file_name("unrelated.png");
    std::fs::write(&unrelated, "keep").unwrap();
    fixture.save();
    let original = fixture.head();
    fixture.configure(
        "max_items = 1\nmax_age_days = 0\nmax_bytes = 0",
        "",
        "max_items = 3",
    );
    aggr_command(&fixture.root)
        .args(["sync", "--dry-run"])
        .assert()
        .success();
    assert!(old.exists());
    assert_eq!(fixture.head(), original);
    fixture.sync();
    assert_eq!(fixture.articles("alpha").len(), 1);
    assert_eq!(fixture.articles("beta").len(), 3);
    assert!(!old.exists());
    assert!(companions.iter().all(|file| !file.exists()));
    assert!(unrelated.exists());
    let relative = old.strip_prefix(fixture.data()).unwrap().to_str().unwrap();
    assert!(
        git(
            &fixture.data(),
            &["show", &format!("{}:{relative}", original.trim())]
        )
        .unwrap()
        .contains("alpha-2")
    );
    git(
        &fixture.data(),
        &["merge-base", "--is-ancestor", original.trim(), "HEAD"],
    )
    .unwrap();
    let after = fixture.head();
    fixture.sync();
    assert_eq!(fixture.head(), after);
}

#[test]
fn byte_limits_count_unreferenced_media_even_in_remote_offline_builds() {
    let fixture = Fixture::new();
    let old = fixture.article("alpha-2");
    let image = companion(&old, ".image-123456789abc.png");
    std::fs::write(&image, vec![0; 20_000]).unwrap();
    fixture.save();
    fixture.configure("max_items = 0\nmax_age_days = 0\nmax_bytes = 10000", "", "");
    let before = fixture.head();
    aggr_command(&fixture.root)
        .args(["build", "--offline"])
        .assert()
        .success();
    let stem = old.file_stem().unwrap().to_str().unwrap();
    assert!(
        !fixture
            .root
            .join(format!("_site/items/alpha/{stem}/index.html"))
            .exists()
    );
    assert_eq!(fixture.head(), before);
    assert!(image.exists());
    fixture.sync();
    assert!(!old.exists());
    assert!(!image.exists());
    assert_eq!(fixture.articles("alpha").len(), 2);
    assert_eq!(fixture.articles("beta").len(), 3);
    let after = fixture.head();
    fixture.sync();
    assert_eq!(fixture.head(), after);
}

#[test]
fn date_cutoff_is_inclusive_and_sources_can_clear_it() {
    let fixture = Fixture::new();
    let date = (Utc::now() - Duration::days(1)).date_naive();
    fixture.configure(
        &format!("max_items = 0\nmax_age_days = 0\nmax_bytes = 0\nsince = {date}"),
        "",
        "since = false",
    );
    fixture.sync();
    assert_eq!(fixture.articles("alpha").len(), 2);
    assert_eq!(fixture.articles("beta").len(), 3);
}

#[test]
fn wider_limits_restore_policy_evictions_but_preserve_manual_deletions() {
    let fixture = Fixture::new();
    let seen = fixture.data().join("sources/alpha/seen.txt");
    let original_seen = std::fs::read(&seen).unwrap();
    fixture.configure(
        "max_items = 1\nmax_age_days = 0\nmax_bytes = 0",
        "",
        "max_items = 3",
    );
    fixture.sync();
    assert_eq!(fixture.articles("alpha").len(), 1);
    let ledger = fixture.data().join("sources/alpha/evicted.json");
    assert!(ledger.exists());

    fixture.configure("max_items = 3\nmax_age_days = 0\nmax_bytes = 0", "", "");
    fixture.sync();
    assert_eq!(fixture.articles("alpha").len(), 3);
    assert_eq!(std::fs::read(&seen).unwrap(), original_seen);
    assert!(!ledger.exists());
    let restored = fixture.head();
    fixture.sync();
    assert_eq!(fixture.head(), restored);

    let removed = fixture.article("alpha-2");
    std::fs::remove_file(&removed).unwrap();
    let html = companion(&removed, ".html");
    if html.exists() {
        std::fs::remove_file(html).unwrap();
    }
    fixture.save();
    fixture.configure("max_items = 4\nmax_age_days = 0\nmax_bytes = 0", "", "");
    fixture.sync();
    assert_eq!(fixture.articles("alpha").len(), 2);
    assert!(!removed.exists());
    assert_eq!(std::fs::read(seen).unwrap(), original_seen);
    assert!(!ledger.exists());
}

#[test]
fn media_backfill_prunes_before_requests_and_enforces_newly_saved_bytes() {
    let fixture = Fixture::new();
    let oldest = fixture.article("alpha-2");
    let middle = fixture.article("alpha-1");
    for (article, image) in [(&oldest, "/pruned.png"), (&middle, "/large.png")] {
        let text = std::fs::read_to_string(article).unwrap();
        std::fs::write(
            article,
            format!("{text}\n![Stored picture]({})\n", fixture.server.url(image)),
        )
        .unwrap();
    }
    fixture.save();
    let newest = fixture.article("alpha-0");
    let retained = std::fs::read(&newest).unwrap();
    fixture.server.reset();
    let feeds = fixture.server.mock(|when, then| {
        when.path_matches(regex::Regex::new("^/(alpha|beta)\\.xml$").unwrap());
        then.status(500);
    });
    let skipped = fixture.server.mock(|when, then| {
        when.path("/pruned.png");
        then.status(500);
    });
    let picture = image::RgbImage::from_fn(640, 400, |x, y| {
        let value = (x + y * 640).wrapping_mul(0x9e3779b9);
        let value = (value ^ (value >> 16)).wrapping_mul(0x85ebca6b);
        image::Rgb([value as u8, (value >> 8) as u8, (value >> 16) as u8])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    picture
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    assert!(bytes.get_ref().len() > 10_000);
    let downloaded = fixture.server.mock(|when, then| {
        when.path("/large.png");
        then.status(200)
            .header("content-type", "image/png")
            .body(bytes.into_inner());
    });
    fixture.configure(
        "media = 'local'\nmax_items = 2\nmax_age_days = 0\nmax_bytes = 10000",
        "",
        "max_items = 3",
    );
    aggr_command(&fixture.root)
        .args(["sync", "--backfill-media"])
        .assert()
        .success();
    feeds.assert_calls(0);
    skipped.assert_calls(0);
    downloaded.assert_calls(1);
    assert!(!oldest.exists());
    assert!(!middle.exists());
    assert_eq!(fixture.articles("alpha").len(), 1);
    assert_eq!(fixture.articles("beta").len(), 3);
    assert_eq!(std::fs::read(newest).unwrap(), retained);
    let after = fixture.head();
    aggr_command(&fixture.root)
        .args(["sync", "--backfill-media"])
        .assert()
        .success();
    assert_eq!(fixture.head(), after);
    downloaded.assert_calls(1);
    feeds.assert_calls(0);
}
