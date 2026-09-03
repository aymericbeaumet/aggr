//! End-to-end: a bare origin, a clone holding `aggr.toml`, feeds served by httpmock, and the
//! real binary. Nothing here touches the network or the user's git configuration.

use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::process::Stdio;
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::{Duration, Instant};

use assert_cmd::prelude::*;
use httpmock::prelude::*;
use predicates::prelude::*;
use tempfile::TempDir;

const FEED: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>Demo blog</title><link>https://demo.example/</link>
<item><title>Hello &lt;b&gt;there&lt;/b&gt;</title><link>https://demo.example/hello</link><guid>g1</guid>
<pubDate>Tue, 01 Sep 2026 10:00:00 GMT</pubDate>
<description><![CDATA[<p>Body <em>one</em> <script>alert(1)</script><img src=x onerror=alert(2)></p>]]></description></item>
<item><title>Second</title><link>https://demo.example/second</link><guid>g2</guid>
<pubDate>Mon, 31 Aug 2026 10:00:00 GMT</pubDate><description>plain</description></item>
</channel></rss>"#;

const FEED_WITH_THIRD: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>Demo blog</title><link>https://demo.example/</link>
<item><title>Third</title><link>https://demo.example/third</link><guid>g3</guid>
<pubDate>Wed, 02 Sep 2026 10:00:00 GMT</pubDate><description>three</description></item>
<item><title>Hello &lt;b&gt;there&lt;/b&gt;</title><link>https://demo.example/hello</link><guid>g1</guid>
<pubDate>Tue, 01 Sep 2026 10:00:00 GMT</pubDate><description>x</description></item>
</channel></rss>"#;

struct TestRepo {
    _tmp: TempDir,
    origin: PathBuf,
    clone: PathBuf,
}

impl TestRepo {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        git(
            tmp.path(),
            &["init", "-q", "--bare", "-b", "main", "origin.git"],
        );
        let clone = tmp.path().join("clone");
        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        Self {
            _tmp: tmp,
            origin,
            clone,
        }
    }

    fn write_config(&self, feed_url: &str, extra: &str) {
        self.write_raw_config(&format!(
            "[site]\ntitle = \"Test reads\"\nrepository = \"o/r\"\ndiscussions = []\n{extra}\n[fetch]\ncontent = \"light\"\n[[sources]]\nurl = \"{feed_url}\"\nname = \"Demo\"\ncategory = \"demo\"\nlabels = [\"example\", \"news\"]\n"
        ));
    }

    fn write_raw_config(&self, config: &str) {
        std::fs::write(self.clone.join("aggr.toml"), config).unwrap();
        git(&self.clone, &["add", "-A"]);
        git(&self.clone, &["commit", "-q", "-m", "config"]);
        git(&self.clone, &["push", "-q", "-u", "origin", "main"]);
    }

    fn aggr(&self) -> Command {
        let mut cmd = Command::cargo_bin("aggr").unwrap();
        cmd.current_dir(&self.clone);
        for (key, value) in git_env() {
            cmd.env(key, value);
        }
        for key in [
            "GITHUB_ACTIONS",
            "GITHUB_REPOSITORY",
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "AGGR_BASE_URL",
            "AGGR_CONFIG",
            "AGGR_CACHE_DIR",
        ] {
            cmd.env_remove(key);
        }
        cmd
    }

    fn origin_rev(&self, rev: &str) -> Option<String> {
        let out = Command::new("git")
            .args(["rev-parse", "--verify", "-q", rev])
            .current_dir(&self.origin)
            .output()
            .unwrap();
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn origin_files(&self, rev: &str) -> Vec<String> {
        let out = Command::new("git")
            .args(["ls-tree", "-r", "--name-only", rev])
            .current_dir(&self.origin)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn origin_log(&self, rev: &str) -> String {
        let out = Command::new("git")
            .args(["log", "--format=%s%n%b", rev])
            .current_dir(&self.origin)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn origin_show(&self, rev: &str, path: &str) -> String {
        let out = Command::new("git")
            .args(["show", &format!("{rev}:{path}")])
            .current_dir(&self.origin)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn data_dir(&self) -> PathBuf {
        self.clone.join(".aggr/data")
    }
}

#[cfg(unix)]
fn wait_for_dev(port: u16, timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("aggr dev did not listen on 127.0.0.1:{port}");
}

#[cfg(unix)]
fn stop_dev(mut child: std::process::Child) -> std::process::Output {
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    panic!("aggr dev did not stop after SIGINT");
}

#[cfg(unix)]
fn wait_for_cached_site(root: &Path, timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name() == ".aggr-site")
        {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("aggr dev did not populate its persistent cache");
}

fn git_env() -> Vec<(&'static str, String)> {
    vec![
        ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("GIT_AUTHOR_NAME", "t".into()),
        ("GIT_AUTHOR_EMAIL", "t@t".into()),
        ("GIT_COMMITTER_NAME", "t".into()),
        ("GIT_COMMITTER_EMAIL", "t@t".into()),
    ]
}

fn git(dir: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    for (key, value) in git_env() {
        cmd.env(key, value);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn init_writes_config_and_workflow() {
    let repo = TestRepo::new();
    repo.aggr()
        .args(["init", "--github"])
        .assert()
        .success()
        .stdout(predicate::str::contains("wrote aggr.toml"));
    assert!(repo.clone.join(".github/workflows/aggr.yml").exists());
    repo.aggr().arg("init").assert().failure();
    repo.aggr()
        .args(["init", "--defaults", "--force"])
        .assert()
        .success();
    let text = std::fs::read_to_string(repo.clone.join("aggr.toml")).unwrap();
    assert!(text.contains("[digest]"));
}

#[cfg(unix)]
#[test]
fn dev_uses_an_external_persistent_cache_and_stops_on_ctrl_c() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/feed.xml");
        then.status(200).body(FEED);
    });
    let repo = TestRepo::new();
    repo.write_config(&server.url("/feed.xml"), "");
    let cache = tempfile::tempdir().unwrap();

    let run = |port: u16| {
        let mut command = repo.aggr();
        command
            .env("AGGR_CACHE_DIR", cache.path())
            .args(["dev", "--port", &port.to_string()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.spawn().unwrap()
    };

    let available = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = available.local_addr().unwrap().port();
    drop(available);
    let first = run(port);
    wait_for_dev(port, Duration::from_secs(10));
    wait_for_cached_site(cache.path(), Duration::from_secs(20));
    let first = stop_dev(first);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let second = run(port);
    wait_for_dev(port, Duration::from_secs(10));
    let second = stop_dev(second);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(
        String::from_utf8_lossy(&second.stdout)
            .contains("restored the previous dev build from cache")
    );

    assert!(!repo.clone.join(".aggr").exists());
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo.clone)
        .output()
        .unwrap();
    assert!(
        status.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&status.stdout)
    );
}

#[test]
fn sync_bootstraps_appends_and_leaves_no_trace_when_nothing_changed() {
    let server = MockServer::start();
    let mut feed = server.mock(|when, then| {
        when.method(GET).path("/feed.xml");
        then.status(200)
            .header("content-type", "application/rss+xml")
            .header("etag", "\"v1\"")
            .body(FEED);
    });
    let repo = TestRepo::new();
    repo.write_config(&server.url("/feed.xml"), "");

    // First sync: orphan branch, README, items, trailers, last-good, pushed.
    repo.aggr()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("demo: +2"))
        .stdout(predicate::str::contains("aggr: init"));
    let tip = repo
        .origin_rev("refs/heads/aggr")
        .expect("data branch pushed");
    assert_eq!(
        repo.origin_rev("refs/aggr/last-good").as_deref(),
        Some(tip.as_str())
    );
    let files = repo.origin_files("aggr");
    assert!(files.contains(&"README.md".to_string()));
    assert!(files.contains(&".gitattributes".to_string()));
    assert!(files.contains(&"items/demo/2026/09/2026-09-01-hello-there.md".to_string()));
    assert!(files.contains(&"items/demo/2026/09/2026-09-01-hello-there.html".to_string()));
    assert!(files.contains(&"items/demo/2026/08/2026-08-31-second.md".to_string()));
    assert!(files.contains(&"sources/demo/seen.txt".to_string()));
    assert!(
        !files.iter().any(|f| f == "status.toml"),
        "no errors, no status file"
    );
    let log = repo.origin_log("aggr");
    assert!(log.contains("Aggr-Version: "), "{log}");
    assert!(log.contains("Aggr-Sources: 1 ok, 0 error"), "{log}");
    assert!(log.contains("Aggr-Config: "), "{log}");
    let md = repo.origin_show("aggr", "items/demo/2026/09/2026-09-01-hello-there.md");
    assert!(md.starts_with("---\ntitle: Hello there\n"), "{md}");
    assert!(md.contains("source: demo"));
    assert!(
        !md.contains("alert("),
        "scripts never reach the markdown: {md}"
    );
    let html = repo.origin_show("aggr", "items/demo/2026/09/2026-09-01-hello-there.html");
    assert!(!html.contains("<script"), "{html}");
    assert!(!html.contains("onerror"), "{html}");
    // main is untouched.
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo.clone)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&status.stdout).trim(), "");

    // Second sync: conditional GET → 304 → nothing new, no commit.
    feed.delete();
    let mut not_modified = server.mock(|when, then| {
        when.method(GET)
            .path("/feed.xml")
            .header("if-none-match", "\"v1\"");
        then.status(304);
    });
    repo.aggr()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("demo: unchanged"))
        .stdout(predicate::str::contains("nothing new"));
    not_modified.assert();
    assert_eq!(
        repo.origin_rev("refs/heads/aggr").as_deref(),
        Some(tip.as_str())
    );

    // A new entry: exactly one more commit, existing files untouched, a stem collision avoided.
    not_modified.delete();
    let mut third = server.mock(|when, then| {
        when.method(GET).path("/feed.xml");
        then.status(200)
            .header("etag", "\"v2\"")
            .body(FEED_WITH_THIRD);
    });
    repo.aggr()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("demo: +1"))
        .stdout(predicate::str::contains("aggr: +1 item"));
    let tip2 = repo.origin_rev("refs/heads/aggr").unwrap();
    assert_ne!(tip, tip2);
    assert_eq!(
        repo.origin_rev("refs/aggr/last-good").as_deref(),
        Some(tip2.as_str())
    );
    let files = repo.origin_files("aggr");
    assert!(files.contains(&"items/demo/2026/09/2026-09-02-third.md".to_string()));
    assert_eq!(
        files
            .iter()
            .filter(|f| f.starts_with("items/") && f.ends_with(".md"))
            .count(),
        3
    );
    let md = repo.origin_show("aggr", "items/demo/2026/09/2026-09-01-hello-there.md");
    assert!(!md.contains("\nx"), "existing item was not rewritten: {md}");

    // Deleting an item on the branch is final: the seen key keeps it out.
    let doomed = repo
        .data_dir()
        .join("items/demo/2026/09/2026-09-02-third.md");
    std::fs::remove_file(&doomed).unwrap();
    std::fs::remove_file(doomed.with_extension("html")).unwrap();
    git(&repo.data_dir(), &["commit", "-qam", "delete third"]);
    git(&repo.data_dir(), &["push", "-q", "origin", "aggr"]);
    let deleted_tip = repo.origin_rev("refs/heads/aggr").unwrap();
    // A different body (so the hash guard does not short-circuit) listing the same entries.
    third.delete();
    server.mock(|when, then| {
        when.method(GET).path("/feed.xml");
        then.status(200)
            .header("etag", "\"v3\"")
            .body(FEED_WITH_THIRD.replace("three", "three, edited"));
    });
    repo.aggr()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("demo: unchanged"))
        .stdout(predicate::str::contains("nothing new"));
    assert_eq!(
        repo.origin_rev("refs/heads/aggr").as_deref(),
        Some(deleted_tip.as_str()),
        "validator changes alone must leave no commit"
    );
    assert!(
        !repo
            .origin_files("aggr")
            .contains(&"items/demo/2026/09/2026-09-02-third.md".to_string())
    );
}

#[test]
fn sync_fetch_only_writes_locally_without_committing_or_pushing() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/feed.xml");
        then.status(200)
            .header("content-type", "application/rss+xml")
            .body(FEED);
    });
    let repo = TestRepo::new();
    repo.write_config(&server.url("/feed.xml"), "");

    repo.aggr()
        .args(["sync", "--fetch-only"])
        .assert()
        .success()
        .stdout(predicate::str::contains("demo: +2"))
        .stdout(predicate::str::contains(
            "fetch only: 2 new item(s), nothing committed or pushed",
        ));

    assert!(
        repo.data_dir()
            .join("items/demo/2026/09/2026-09-01-hello-there.md")
            .is_file()
    );
    assert!(repo.origin_rev("refs/heads/aggr").is_none());
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo.clone)
        .output()
        .unwrap();
    assert!(status.stdout.is_empty());
}

#[test]
fn source_errors_are_recorded_on_transition_only_and_all_failed_is_fatal() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/ok.xml");
        then.status(200).body(FEED);
    });
    server.mock(|when, then| {
        when.method(GET).path("/broken.xml");
        then.status(500);
    });
    let repo = TestRepo::new();
    let config = format!(
        "[site]\ntitle = \"T\"\n[fetch]\nretries = 0\ncontent = \"light\"\n[[sources]]\nurl = \"{}\"\nname = \"ok\"\n[[sources]]\nurl = \"{}\"\nname = \"broken\"\n",
        server.url("/ok.xml"),
        server.url("/broken.xml")
    );
    std::fs::write(repo.clone.join("aggr.toml"), config).unwrap();
    git(&repo.clone, &["add", "-A"]);
    git(&repo.clone, &["commit", "-qm", "config"]);
    git(&repo.clone, &["push", "-q", "-u", "origin", "main"]);

    repo.aggr()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("broken: error"));
    let status = repo.origin_show("aggr", "status.toml");
    assert!(status.contains("[errors.broken]"), "{status}");
    assert!(status.contains("500"), "{status}");
    assert!(
        repo.origin_rev("refs/aggr/last-good").is_none(),
        "an error means no last-good"
    );
    let log = repo.origin_log("aggr");
    assert!(log.contains("Aggr-Sources: 1 ok, 1 error"), "{log}");
    let tip = repo.origin_rev("refs/heads/aggr").unwrap();

    // Same failure again: nothing to commit.
    repo.aggr().arg("sync").assert().success();
    assert_eq!(repo.origin_rev("refs/heads/aggr").unwrap(), tip);

    // Every source failing is the one fetch condition that fails the run.
    let config = format!(
        "[site]\ntitle = \"T\"\n[fetch]\nretries = 0\ncontent = \"light\"\n[[sources]]\nurl = \"{}\"\nname = \"broken\"\n",
        server.url("/broken.xml")
    );
    std::fs::write(repo.clone.join("aggr.toml"), config).unwrap();
    repo.aggr()
        .arg("sync")
        .assert()
        .failure()
        .stderr(predicate::str::contains("every source failed"));
}

#[test]
fn build_renders_the_site_and_release_needs_a_url() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/feed.xml");
        then.status(200).body(FEED);
    });
    let repo = TestRepo::new();
    repo.write_config(&server.url("/feed.xml"), "");

    repo.aggr()
        .arg("build")
        .assert()
        .success()
        .stdout(predicate::str::contains("demo: +2"))
        .stdout(predicate::str::contains("2 item(s)"));
    let site = repo.clone.join("_site");
    let index = std::fs::read_to_string(site.join("index.html")).unwrap();
    assert!(
        index.contains("href=\"items/demo/2026-09-01-hello-there/\""),
        "{index}"
    );
    assert!(index.contains("Hello there"));
    assert!(index.contains(">aggr.toml ↗</a>"));
    assert!(index.contains(">built with aggr</a>"));
    assert!(index.contains("href=\"https://github.com/aymericbeaumet/aggr\""));
    assert!(index.contains("href=\"sources/\""), "{index}");
    assert!(index.contains("href=\"preferences/\""), "{index}");
    assert!(index.contains("target=\"_blank\""), "{index}");
    assert!(!index.contains("built <time"));
    assert!(index.contains("id=\"swup\""));
    assert!(
        index.find("<header class=\"top\">").unwrap() < index.find("<main id=\"swup\"").unwrap(),
        "the persistent menubar must stay outside Swup's replacement container"
    );
    assert!(index.contains("assets/swup-"));
    assert!(!index.contains("config@"));
    assert!(!index.contains("data@"));
    assert!(!index.contains("starred"));
    assert!(!index.contains("unread"));
    assert!(!index.contains("&#x2f;"));
    let item = site.join("items/demo/2026-09-01-hello-there");
    let page = std::fs::read_to_string(item.join("index.html")).unwrap();
    assert!(!page.contains("https://github.com/o/r/blob/"), "{page}");
    assert!(!page.contains("alert("), "{page}");
    let representation = site.join("items/demo/2026-09-01-hello-there");
    assert!(representation.with_extension("md").exists());
    assert!(representation.with_extension("txt").exists());
    assert!(representation.with_extension("rst").exists());
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(representation.with_extension("json")).unwrap())
            .unwrap();
    assert_eq!(json["title"], "Hello there");
    assert_eq!(json["source"], "demo");
    assert!(!item.join("html.html").exists());
    assert!(page.contains(">original</a>"), "{page}");
    let category = page.find(">demo</a>").unwrap();
    let first_tag = page.find(">#example</a>").unwrap();
    assert!(
        category < first_tag,
        "tags must follow the category: {page}"
    );
    assert!(
        page.contains("<time class=\"dt-published\" datetime=\"2026-09-01T"),
        "{page}"
    );
    assert!(page.contains("title=\"2026-09-01T"), "{page}");
    assert!(!page.contains("blob "), "{page}");
    assert!(site.join("pagefind/pagefind.js").exists());
    assert!(site.join("feed.xml").exists());
    assert!(site.join("atom.xml").exists());
    assert!(site.join("rss.xml").exists());
    assert!(site.join("feed.json").exists());
    assert!(site.join(".nojekyll").exists());
    assert!(site.join("sources/demo/index.html").exists());
    assert!(site.join("preferences/index.html").exists());
    assert!(!site.join("settings/index.html").exists());
    assert!(site.join("aggr.toml").exists());
    assert!(site.join("categories/demo/index.html").exists());
    assert!(site.join("categories/demo/atom.xml").exists());
    assert!(site.join("categories/demo/rss.xml").exists());
    assert!(site.join("categories/demo/feed.json").exists());
    assert!(site.join("categories/index.html").exists());
    assert!(site.join("categories/atom.xml").exists());
    assert!(site.join("categories/rss.xml").exists());
    assert!(site.join("categories/feed.json").exists());
    assert!(site.join("tags/index.html").exists());
    assert!(site.join("tags/atom.xml").exists());
    assert!(site.join("tags/rss.xml").exists());
    assert!(site.join("tags/feed.json").exists());
    assert!(site.join("tags/example/index.html").exists());
    assert!(site.join("tags/example/atom.xml").exists());
    assert!(site.join("tags/example/rss.xml").exists());
    assert!(site.join("tags/example/feed.json").exists());
    let tags = std::fs::read_to_string(site.join("tags/index.html")).unwrap();
    assert!(tags.contains(">#example</a>"), "{tags}");
    let tag = std::fs::read_to_string(site.join("tags/example/index.html")).unwrap();
    let tag = tag.replace("\r\n", "\n");
    assert!(tag.contains("<h1>\n      #example\n"), "{tag}");
    let search = std::fs::read_to_string(site.join("search/index.html")).unwrap();
    assert!(search.contains(">#example (2)</option>"), "{search}");
    assert!(!site.join("CNAME").exists());
    // Installable and readable offline: manifest, worker precaching the newest pages, fallback.
    assert!(index.contains("rel=\"manifest\""), "{index}");
    let sw = std::fs::read_to_string(site.join("sw.js")).unwrap();
    assert!(sw.contains("\"assets/style-"), "{sw}");
    assert!(sw.contains("\"assets/swup-"), "{sw}");
    assert!(
        sw.contains("\"items/demo/2026-09-01-hello-there/\""),
        "{sw}"
    );
    assert!(site.join("manifest.webmanifest").exists());
    assert!(site.join("offline.html").exists());
    // The output directory never shows up on main.
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo.clone)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&status.stdout).trim(), "");

    // Build owns its required sync, and an unchanged second run reuses rendered output.
    repo.aggr()
        .arg("build")
        .assert()
        .success()
        .stdout(predicate::str::contains("demo: unchanged"))
        .stdout(predicate::str::contains("from cache"));

    repo.aggr()
        .args(["build", "--release"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--release needs a public URL"));
    repo.aggr()
        .args(["build", "--release", "--base-url", "https://o.github.io/r/"])
        .assert()
        .success();
    let index = std::fs::read_to_string(site.join("index.html")).unwrap();
    assert!(index.contains("href=\"items/demo/"), "{index}");
    assert!(
        !site.join("CNAME").exists(),
        "github.io hosts need no CNAME"
    );
    let manifest = std::fs::read_to_string(site.join("manifest.webmanifest")).unwrap();
    assert!(manifest.contains("\"start_url\": \"./\""), "{manifest}");

    repo.write_config(
        &server.url("/feed.xml"),
        "url = \"https://reads.example.com\"\npwa = false",
    );
    repo.aggr().args(["build", "--release"]).assert().success();
    assert!(!site.join("sw.js").exists(), "pwa = false writes no worker");
    assert!(!site.join("manifest.webmanifest").exists());
    assert_eq!(
        std::fs::read_to_string(site.join("CNAME")).unwrap(),
        "reads.example.com"
    );
    let feed = std::fs::read_to_string(site.join("feed.xml")).unwrap();
    assert!(
        feed.contains("https://reads.example.com/items/demo/"),
        "{feed}"
    );

    // Any ref of the data branch can be rendered.
    repo.aggr()
        .args([
            "build",
            "--data-ref",
            "refs/aggr/last-good",
            "--out",
            "elsewhere",
        ])
        .assert()
        .success();
    assert!(repo.clone.join("elsewhere/index.html").exists());
}

#[test]
fn check_and_digest_dry_run() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/feed.xml");
        then.status(200).body(FEED);
    });
    let repo = TestRepo::new();
    repo.write_config(
        &server.url("/feed.xml"),
        "url = \"https://reads.example.com\"\n[digest]\nat = \"00:00\"\n",
    );
    repo.aggr()
        .arg("check")
        .assert()
        .success()
        .stdout(predicate::str::contains("ok     demo"))
        .stdout(predicate::str::contains("2 item(s)"));

    repo.aggr().arg("sync").assert().success();
    repo.aggr()
        .args(["digest", "--dry-run", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Digest #1 · "))
        .stdout(predicate::str::contains("· 2 new"))
        .stdout(predicate::str::contains("- [Hello there](https://demo.example/hello) · [source](https://reads.example.com/items/demo/2026-09-01-hello-there/) · [md](https://github.com/o/r/blob/"));
    // Posting needs a token; without one the command fails loudly instead of silently skipping.
    repo.aggr()
        .args(["digest", "--force"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("GITHUB_TOKEN"));
}

#[test]
fn store_retention_prunes_the_tree_but_keeps_seen_keys() {
    let server = MockServer::start();
    let mut feed = server.mock(|when, then| {
        when.method(GET).path("/feed.xml");
        then.status(200).body(FEED);
    });
    let repo = TestRepo::new();
    repo.write_config(&server.url("/feed.xml"), "[store]\nmax_items = 1\n");

    repo.aggr()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("demo: +2"))
        .stdout(predicate::str::contains("retention: -1"));
    let files = repo.origin_files("aggr");
    let items: Vec<&String> = files.iter().filter(|f| f.starts_with("items/")).collect();
    assert_eq!(
        items,
        vec![
            "items/demo/2026/09/2026-09-01-hello-there.html",
            "items/demo/2026/09/2026-09-01-hello-there.md"
        ],
        "only the newest item stays in the tree"
    );
    let seen = repo.origin_show("aggr", "sources/demo/seen.txt");
    assert!(
        seen.lines().count() >= 4,
        "both items' keys are kept:\n{seen}"
    );

    // The pruned item never comes back, even when the feed body changes.
    feed.delete();
    server.mock(|when, then| {
        when.method(GET).path("/feed.xml");
        then.status(200)
            .body(FEED.replace("plain", "plain, edited"));
    });
    repo.aggr()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("demo: unchanged"))
        .stdout(predicate::str::contains("nothing new"));
}

const LISTING: &str = r#"<!doctype html><html><head><title>Blog | Scraped</title></head><body><ul>
<li><a class="card" href="/blog/newest"><h2>Newest post</h2><span class="mono">08.23.26</span><p>What is new</p></a></li>
<li><a class="card" href="/blog/older"><h2>Older post</h2><span class="mono">07.22.26</span><p>What was new</p></a></li>
</ul></body></html>"#;

#[test]
fn included_topic_files_and_automatic_html_fallback_work_end_to_end() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/feed.xml");
        then.status(200).body(FEED);
    });
    server.mock(|when, then| {
        when.method(GET).path("/blog");
        then.status(200)
            .header("content-type", "text/html; charset=utf-8")
            .body(LISTING);
    });
    let repo = TestRepo::new();
    std::fs::write(
        repo.clone.join("aggr-ai.toml"),
        format!(
            "[[sources]]\nname = \"Scraped\"\nurl = \"{}\"\n",
            server.url("/blog")
        ),
    )
    .unwrap();
    repo.write_raw_config(&format!(
        "[site]\ntitle = \"Test reads\"\nrepository = \"o/r\"\n\
         [[sources]]\nurl = \"./aggr-*.toml\"\ncategory = \"ai\"\n\
         [[sources]]\nurl = \"{}\"\nname = \"Demo\"\ncategory = \"demo\"\n",
        server.url("/feed.xml")
    ));

    repo.aggr()
        .arg("check")
        .assert()
        .success()
        .stdout(predicate::str::contains("ok     demo"))
        .stdout(predicate::str::contains("ok     scraped  web"))
        .stdout(predicate::str::contains("2 item(s)  \"Blog | Scraped\""));

    repo.aggr()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("demo: +2"))
        .stdout(predicate::str::contains("scraped: +2"));
    let files = repo.origin_files("aggr");
    assert!(
        files.contains(&"items/scraped/2026/08/2026-08-23-newest-post.md".to_string()),
        "{files:?}"
    );
    assert!(
        !files.contains(&"items/scraped/2026/08/2026-08-23-newest-post.html".to_string()),
        "listings carry no body html"
    );
    let item = repo.origin_show("aggr", "items/scraped/2026/08/2026-08-23-newest-post.md");
    assert!(
        item.contains(&format!("link: {}/blog/newest", server.url(""))),
        "relative links resolve against the page:\n{item}"
    );
    assert!(item.contains("summary: What is new"), "{item}");
    assert!(
        item.trim_end().ends_with("---\n\nWhat is new"),
        "summary is the body:\n{item}"
    );

    repo.aggr().arg("build").assert().success();
    let category =
        std::fs::read_to_string(repo.clone.join("_site/categories/ai/index.html")).unwrap();
    assert!(
        category.contains("Newest post"),
        "file-level category applies"
    );

    // Unchanged listing: no new items, no commit.
    let before = repo.origin_rev("aggr").unwrap();
    repo.aggr()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("scraped: unchanged"));
    assert_eq!(repo.origin_rev("aggr").unwrap(), before);
}
