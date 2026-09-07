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
use sha1::Digest as _;
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
            "[site]\ntitle = \"Test reads\"\nrepository = \"o/r\"\n{extra}\n[fetch]\ncontent = \"light\"\nimages = false\n[[sources]]\nurl = \"{feed_url}\"\nname = \"Demo\"\ncategory = \"demo\"\nlabels = [\"example\", \"news\"]\n"
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

    fn origin_bytes(&self, rev: &str, path: &str) -> Vec<u8> {
        let out = Command::new("git")
            .args(["show", &format!("{rev}:{path}")])
            .current_dir(&self.origin)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
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
            .any(|entry| {
                entry.file_name() == ".aggr-site"
                    && entry.path().parent().is_some_and(|site| {
                        site.file_name().is_some_and(|name| name == "site")
                            && site.join(".aggr-dev-key").is_file()
                            && entry.path().strip_prefix(root).is_ok_and(|relative| {
                                !relative.components().any(|part| part.as_os_str() == "tmp")
                            })
                    })
            })
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

fn preview_image() -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(640, 400, image::Rgb([48, 128, 96]))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn preview_feed(server: &MockServer, entries: &[(&str, &str, &str)]) -> String {
    serde_json::json!({
        "version": "https://jsonfeed.org/version/1.1",
        "title": "Preview feed",
        "items": entries.iter().map(|(id, image, published)| serde_json::json!({
            "id": id,
            "title": id,
            "url": server.url(format!("/articles/{id}")),
            "date_published": published,
            "content_html": "<p>Article body with <strong>useful content</strong>.</p>",
            "image": server.url(*image),
        })).collect::<Vec<_>>()
    })
    .to_string()
}

fn item_front(repo: &TestRepo, rev: &str, title: &str) -> (String, serde_yaml_ng::Value) {
    let path = repo
        .origin_files(rev)
        .into_iter()
        .find(|path| path.starts_with("items/") && path.ends_with(&format!("-{title}.md")))
        .unwrap();
    let markdown = repo.origin_show(rev, &path);
    let front = markdown
        .strip_prefix("---\n")
        .unwrap()
        .split_once("\n---\n")
        .unwrap()
        .0;
    (path, serde_yaml_ng::from_str(front).unwrap())
}

#[test]
fn previews_are_new_only_optional_and_preserved_on_refresh() {
    let server = MockServer::start();
    let mut feed = server.mock(|when, then| {
        when.method(GET).path("/feed.json");
        then.status(200)
            .header("content-type", "application/feed+json")
            .body(preview_feed(
                &server,
                &[("old", "/old.png", "2026-09-01T10:00:00Z")],
            ));
    });
    let old_image = server.mock(|when, then| {
        when.method(GET).path("/old.png");
        then.status(200).body(preview_image());
    });
    let image = server.mock(|when, then| {
        when.method(GET).path("/new.png");
        then.status(200)
            .header("content-type", "image/png")
            .body(preview_image());
    });
    let missing = server.mock(|when, then| {
        when.method(GET).path("/missing.png");
        then.status(404);
    });
    let repo = TestRepo::new();
    let config = |previews| {
        format!(
            "[fetch]\ncontent = \"light\"\npreviews = {previews}\n[[sources]]\nname = \"Demo\"\nurl = \"{}\"\n",
            server.url("/feed.json"),
        )
    };
    repo.write_raw_config(&config(false));
    repo.aggr().arg("sync").assert().success();
    old_image.assert_calls(0);
    assert!(item_front(&repo, "aggr", "old").1["preview"].is_null());

    repo.write_raw_config(&config(true));
    feed.delete();
    server.mock(|when, then| {
        when.method(GET).path("/feed.json");
        then.status(200)
            .header("content-type", "application/feed+json")
            .body(preview_feed(
                &server,
                &[
                    ("missing", "/missing.png", "2026-09-03T10:00:00Z"),
                    ("new", "/new.png", "2026-09-02T10:00:00Z"),
                    ("old", "/old.png", "2026-09-01T10:00:00Z"),
                ],
            ));
    });
    repo.aggr()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("demo: +2"));
    image.assert_calls(1);
    missing.assert_calls(1);
    old_image.assert_calls(0);
    assert!(item_front(&repo, "aggr", "missing").1["preview"].is_null());
    let (path, front) = item_front(&repo, "aggr", "new");
    let preview = &front["preview"];
    assert_eq!(preview["width"].as_u64(), Some(256));
    assert_eq!(preview["height"].as_u64(), Some(160));
    let companion = Path::new(&path)
        .parent()
        .unwrap()
        .join(preview["file"].as_str().unwrap());
    let bytes = repo.origin_bytes("aggr", companion.to_str().unwrap());
    assert!(bytes.len() <= 384 * 1024);
    assert_eq!(
        image::guess_format(&bytes).unwrap(),
        image::ImageFormat::WebP
    );

    let tip = repo.origin_rev("aggr").unwrap();
    repo.aggr()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing new"));
    assert_eq!(repo.origin_rev("aggr").unwrap(), tip);
    image.assert_calls(1);
    missing.assert_calls(1);

    repo.aggr().args(["sync", "--refresh"]).assert().success();
    assert_eq!(item_front(&repo, "aggr", "new").1["preview"], *preview);
    assert_eq!(
        repo.origin_bytes("aggr", companion.to_str().unwrap()),
        bytes
    );
    assert!(item_front(&repo, "aggr", "old").1["preview"].is_null());
    assert!(item_front(&repo, "aggr", "missing").1["preview"].is_null());
    old_image.assert_calls(0);
    image.assert_calls(1);
    missing.assert_calls(1);
}

#[test]
fn one_download_can_supply_the_article_image_and_its_feed_preview() {
    let server = MockServer::start();
    let source = server.url("/cover.png");
    server.mock(|when, then| {
        when.method(GET).path("/feed.json");
        then.status(200)
            .header("content-type", "application/feed+json")
            .json_body(serde_json::json!({
                "version": "https://jsonfeed.org/version/1.1",
                "title": "Image feed",
                "items": [{
                    "id": "shared-image",
                    "title": "Shared image",
                    "url": server.url("/articles/shared-image"),
                    "date_published": "2026-09-04T10:00:00Z",
                    "image": source,
                    "content_html": format!(
                        "<p>Before.</p><img src=\"{source}\" alt=\"Cover\"><p>After.</p>"
                    ),
                }],
            }));
    });
    let image = server.mock(|when, then| {
        when.method(GET).path("/cover.png");
        then.status(200)
            .header("content-type", "image/png")
            .body(preview_image());
    });
    let repo = TestRepo::new();
    repo.write_raw_config(&format!(
        "[fetch]\ncontent = \"light\"\nimages = true\npreviews = true\n[[sources]]\nname = \"Demo\"\nurl = \"{}\"\n",
        server.url("/feed.json")
    ));

    repo.aggr().arg("sync").assert().success();

    image.assert_calls(1);
    let (_, front) = item_front(&repo, "aggr", "shared-image");
    assert!(!front["preview"].is_null());
    assert_eq!(front["images"].as_sequence().map(Vec::len), Some(1));
}

#[test]
fn article_images_keep_exact_masters_and_publish_lossless_responsive_assets() {
    let server = MockServer::start();
    let source = server.url("/diagram.png");
    let feed = serde_json::json!({
        "version": "https://jsonfeed.org/version/1.1",
        "title": "Image feed",
        "items": [{
            "id": "illustrated",
            "title": "Illustrated article",
            "url": server.url("/articles/illustrated"),
            "date_published": "2026-09-04T10:00:00Z",
            "content_html": format!(
                "<p>Before.</p><img src=\"{source}\" alt=\"A useful diagram\"><p>After.</p>"
            ),
        }],
    });
    server.mock(|when, then| {
        when.method(GET).path("/feed.json");
        then.status(200)
            .header("content-type", "application/feed+json")
            .json_body(feed);
    });
    let master = preview_image();
    let image = server.mock(|when, then| {
        when.method(GET).path("/diagram.png");
        then.status(200)
            .header("content-type", "image/png")
            .body(master.clone());
    });
    let repo = TestRepo::new();
    repo.write_raw_config(&format!(
        "[fetch]\ncontent = \"light\"\nimages = true\n[[sources]]\nname = \"Demo\"\nurl = \"{}\"\n",
        server.url("/feed.json")
    ));

    repo.aggr().arg("sync").assert().success();
    image.assert_calls(1);
    let (path, front) = item_front(&repo, "aggr", "illustrated-article");
    let archived = &front["images"][0];
    let directory = Path::new(&path).parent().unwrap();
    let original = directory.join(archived["original"]["file"].as_str().unwrap());
    assert_eq!(
        repo.origin_bytes("aggr", original.to_str().unwrap()),
        master
    );
    let variants = archived["variants"].as_sequence().unwrap();
    assert!(!variants.is_empty());
    let full = variants.last().unwrap();
    assert_eq!(
        full["width"].as_u64(),
        archived["original"]["width"].as_u64()
    );
    let full_path = directory.join(full["file"].as_str().unwrap());
    let full_bytes = repo.origin_bytes("aggr", full_path.to_str().unwrap());
    assert_eq!(
        image::guess_format(&full_bytes).unwrap(),
        image::ImageFormat::WebP
    );
    assert_eq!(
        image::load_from_memory(&full_bytes).unwrap().to_rgba8(),
        image::load_from_memory(&master).unwrap().to_rgba8()
    );
    assert!(repo.origin_show("aggr", &path).contains(&source));

    repo.aggr().arg("build").assert().success();
    let source_slug = path.split('/').nth(1).unwrap();
    let item_slug = Path::new(&path).file_stem().unwrap();
    let page = std::fs::read_to_string(
        repo.clone
            .join("_site/items")
            .join(source_slug)
            .join(item_slug)
            .join("index.html"),
    )
    .unwrap();
    assert!(
        page.contains("<picture class=\"article-picture\""),
        "{page}"
    );
    assert!(page.contains("data-placeholder=\"assets/images/"), "{page}");
    let expected_ratio = format!(
        "--image-ratio:{} / {}",
        archived["original"]["width"].as_u64().unwrap(),
        archived["original"]["height"].as_u64().unwrap()
    );
    assert!(page.contains(&expected_ratio), "{page}");
    assert!(page.contains(" 48w"), "{page}");
    assert!(page.contains("type=\"image/webp\""), "{page}");
    assert!(page.contains("loading=\"eager\""), "{page}");
    assert!(page.contains("fetchpriority=\"high\""), "{page}");
    let master_asset = format!(
        "assets/images/{}.png",
        hex::encode(sha1::Sha1::digest(&master))
    );
    assert_eq!(
        std::fs::read(repo.clone.join("_site").join(&master_asset)).unwrap(),
        master
    );
    assert!(page.contains(&master_asset), "{page}");
    assert!(
        std::fs::read_to_string(repo.clone.join("_site/sw.js"))
            .unwrap()
            .contains(&master_asset)
    );

    let tip = repo.origin_rev("aggr").unwrap();
    repo.aggr()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing new"));
    assert_eq!(repo.origin_rev("aggr").unwrap(), tip);
    image.assert_calls(1);

    repo.aggr().args(["sync", "--refresh"]).assert().success();
    let (refreshed_path, refreshed) = item_front(&repo, "aggr", "illustrated-article");
    assert_eq!(refreshed["images"], front["images"]);
    assert_eq!(
        repo.origin_bytes(
            "aggr",
            Path::new(&refreshed_path)
                .parent()
                .unwrap()
                .join(refreshed["images"][0]["original"]["file"].as_str().unwrap())
                .to_str()
                .unwrap()
        ),
        master
    );
    image.assert_calls(1);
}

#[test]
fn previews_mirror_local_bytes_and_retention_preserves_historical_blobs() {
    let server = MockServer::start();
    let mut feed = server.mock(|when, then| {
        when.method(GET).path("/feed.json");
        then.status(200)
            .header("content-type", "application/feed+json")
            .body(preview_feed(
                &server,
                &[("old", "/old.png", "2026-09-01T10:00:00Z")],
            ));
    });
    let image = server.mock(|when, then| {
        when.method(GET).path("/old.png");
        then.status(200).body(preview_image());
    });
    let publisher = server.mock(|when, then| {
        when.method(GET).path("/articles/old");
        then.status(500);
    });
    let upstream = TestRepo::new();
    upstream.write_raw_config(&format!(
        "[fetch]\ncontent = \"light\"\npreviews = true\n[store]\nmax_items = 1\n[[sources]]\nname = \"Demo\"\nurl = \"{}\"\n",
        server.url("/feed.json"),
    ));
    upstream.aggr().arg("sync").assert().success();
    let original_tip = upstream.origin_rev("aggr").unwrap();
    let (path, front) = item_front(&upstream, "aggr", "old");
    let companion = Path::new(&path)
        .parent()
        .unwrap()
        .join(front["preview"]["file"].as_str().unwrap());
    let bytes = upstream.origin_bytes("aggr", companion.to_str().unwrap());
    image.assert_calls(1);

    let replica = TestRepo::new();
    replica.write_raw_config(
        "[fetch]\ncontent = \"heavy\"\npreviews = true\n[[sources]]\ntype = \"aggr\"\nname = \"Mirror\"\nurl = \"https://mirror.invalid/source.git\"\n",
    );
    let local = url::Url::from_file_path(&upstream.origin).unwrap();
    let mirror_sync = || {
        let mut cmd = replica.aggr();
        cmd.env("GIT_CONFIG_COUNT", "2")
            .env("GIT_CONFIG_KEY_0", format!("url.{local}.insteadOf"))
            .env("GIT_CONFIG_VALUE_0", "https://mirror.invalid/source.git")
            .env("GIT_CONFIG_KEY_1", "protocol.file.allow")
            .env("GIT_CONFIG_VALUE_1", "always")
            .arg("sync");
        cmd
    };
    mirror_sync()
        .assert()
        .success()
        .stdout(predicate::str::contains("mirror: +1"));
    let (mirror_path, mirror_front) = item_front(&replica, "aggr", "old");
    let mirror_companion = Path::new(&mirror_path)
        .parent()
        .unwrap()
        .join(mirror_front["preview"]["file"].as_str().unwrap());
    assert_eq!(
        replica.origin_bytes("aggr", mirror_companion.to_str().unwrap()),
        bytes
    );
    assert_eq!(mirror_front["preview"], front["preview"]);
    image.assert_calls(1);
    publisher.assert_calls(0);
    let mirror_tip = replica.origin_rev("aggr").unwrap();
    mirror_sync().assert().success();
    assert_eq!(replica.origin_rev("aggr").unwrap(), mirror_tip);
    image.assert_calls(1);
    publisher.assert_calls(0);

    feed.delete();
    server.mock(|when, then| {
        when.method(GET).path("/feed.json");
        then.status(200)
            .header("content-type", "application/feed+json")
            .body(preview_feed(
                &server,
                &[("new", "/old.png", "2026-09-02T10:00:00Z")],
            ));
    });
    upstream
        .aggr()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("retention: -1"));
    let files = upstream.origin_files("aggr");
    assert!(!files.contains(&path));
    assert!(!files.contains(&companion.to_str().unwrap().to_string()));
    assert_eq!(upstream.origin_rev("aggr^").unwrap(), original_tip);
    assert_eq!(
        upstream.origin_bytes(&original_tip, companion.to_str().unwrap()),
        bytes
    );
    assert!(
        upstream
            .origin_show(&original_tip, &path)
            .contains("preview:")
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
    let workflow = std::fs::read_to_string(repo.clone.join(".github/workflows/aggr.yml")).unwrap();
    assert!(workflow.contains("\"**/*.toml\""), "{workflow}");
    assert!(!workflow.contains("branches: [main]"), "{workflow}");
    assert!(
        workflow.contains("github.ref_name == github.event.repository.default_branch"),
        "{workflow}"
    );
    repo.aggr().arg("init").assert().failure();
    repo.aggr()
        .args(["init", "--defaults", "--force"])
        .assert()
        .success();
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
    let cache_path = cache.path().canonicalize().unwrap();

    let run = |port: u16| {
        let mut command = repo.aggr();
        command
            .env("AGGR_CACHE_DIR", &cache_path)
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
    wait_for_cached_site(&cache_path, Duration::from_secs(20));
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
    assert!(index.contains("href=\"library/\""), "{index}");
    assert!(index.contains("href=\"preferences/\""), "{index}");
    assert!(index.contains("target=\"_blank\""), "{index}");
    assert!(!index.contains("built <time"));
    assert!(index.contains("id=\"swup\""));
    assert!(
        index.find("<header class=\"top\"").unwrap() < index.find("<main id=\"swup\"").unwrap(),
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
    assert_eq!(
        page.matches("https://github.com/o/r/blob/").count(),
        1,
        "{page}"
    );
    assert!(page.contains("/aggr.toml\" target=\"_blank\""), "{page}");
    assert!(!page.contains("Git record <code>"), "{page}");
    assert!(page.contains("class=\"article-footer\""), "{page}");
    assert!(page.contains(">Continue reading</h2>"), "{page}");
    assert!(!page.contains("class=\"article-navigation\""), "{page}");
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
    assert_eq!(
        json["@type"],
        serde_json::json!(["WebPage", "ArchiveComponent"])
    );
    assert_eq!(json["mainEntity"]["@id"], "https://demo.example/hello");
    assert_eq!(
        json["git"]["path"],
        "items/demo/2026/09/2026-09-01-hello-there.md"
    );
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
    assert!(site.join("library/index.html").exists());
    assert!(site.join("browse/index.html").exists());
    assert!(site.join("sources/demo/index.html").exists());
    assert!(site.join("sources/index.html").exists());
    assert!(!site.join("sources/atom.xml").exists());
    assert!(!site.join("sources/rss.xml").exists());
    assert!(!site.join("sources/feed.json").exists());
    assert!(site.join("preferences/index.html").exists());
    assert!(!site.join("settings/index.html").exists());
    assert!(site.join("aggr.toml").exists());
    assert!(site.join("categories/demo/index.html").exists());
    assert!(site.join("categories/demo/atom.xml").exists());
    assert!(site.join("categories/demo/rss.xml").exists());
    assert!(site.join("categories/demo/feed.json").exists());
    assert!(site.join("categories/index.html").exists());
    assert!(!site.join("categories/atom.xml").exists());
    assert!(!site.join("categories/rss.xml").exists());
    assert!(!site.join("categories/feed.json").exists());
    assert!(site.join("tags/index.html").exists());
    assert!(!site.join("tags/atom.xml").exists());
    assert!(!site.join("tags/rss.xml").exists());
    assert!(!site.join("tags/feed.json").exists());
    assert!(site.join("tags/example/index.html").exists());
    assert!(site.join("tags/example/atom.xml").exists());
    assert!(site.join("tags/example/rss.xml").exists());
    assert!(site.join("tags/example/feed.json").exists());
    let library = std::fs::read_to_string(site.join("library/index.html")).unwrap();
    assert!(library.contains("id=\"sources\""), "{library}");
    assert!(library.contains("id=\"categories\""), "{library}");
    assert!(library.contains("id=\"tags\""), "{library}");
    assert!(library.contains("href=\"sources/demo/\""), "{library}");
    assert!(library.contains("href=\"categories/demo/\""), "{library}");
    assert!(library.contains("href=\"tags/example/\""), "{library}");
    assert!(library.contains(">#example</a>"), "{library}");
    assert!(!library.contains("explore-nav"), "{library}");
    assert!(!library.contains("directory-count"), "{library}");
    assert!(!library.contains("directory-status"), "{library}");
    for legacy in ["explore", "browse", "sources", "categories", "tags"] {
        let redirect = std::fs::read_to_string(site.join(legacy).join("index.html")).unwrap();
        assert!(redirect.contains("noindex,follow"), "{redirect}");
        assert!(redirect.contains("url=/library/"), "{redirect}");
    }
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
fn build_data_ref_is_offline_and_side_effect_free() {
    let server = MockServer::start();
    let mut feed = server.mock(|when, then| {
        when.method(GET).path("/feed.xml");
        then.status(200).body(FEED);
    });
    let repo = TestRepo::new();
    repo.write_raw_config(&format!(
        "[site]\ntitle = \"Test reads\"\nrepository = \"o/r\"\n\
         [fetch]\ncontent = \"light\"\nretries = 0\n\
         [[sources]]\nurl = \"{}\"\nname = \"Demo\"\n",
        server.url("/feed.xml")
    ));

    repo.aggr().arg("sync").assert().success();
    let pinned = repo.origin_rev("refs/heads/aggr").unwrap();

    feed.delete();
    let mut newer = server.mock(|when, then| {
        when.method(GET).path("/feed.xml");
        then.status(200).body(FEED_WITH_THIRD);
    });
    repo.aggr()
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("demo: +1"));
    let latest = repo.origin_rev("refs/heads/aggr").unwrap();
    assert_ne!(pinned, latest);

    newer.delete();
    let offline = server.mock(|when, then| {
        when.method(GET).path("/feed.xml");
        then.status(503);
    });
    let log_before = repo.origin_log("refs/heads/aggr");

    repo.aggr()
        .args([
            "build",
            "--data-ref",
            pinned.as_str(),
            "--out",
            "pinned-site",
        ])
        .assert()
        .success();

    offline.assert_calls(0);
    assert_eq!(
        repo.origin_rev("refs/heads/aggr").as_deref(),
        Some(latest.as_str())
    );
    assert_eq!(
        repo.origin_rev("refs/aggr/last-good").as_deref(),
        Some(latest.as_str())
    );
    assert_eq!(repo.origin_log("refs/heads/aggr"), log_before);
    let index = std::fs::read_to_string(repo.clone.join("pinned-site/index.html")).unwrap();
    assert!(index.contains("Hello there"), "{index}");
    assert!(index.contains("Second"), "{index}");
    assert!(!index.contains("Third"), "{index}");
}

#[test]
fn check_probes_sources() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/feed.xml");
        then.status(200).body(FEED);
    });
    let repo = TestRepo::new();
    repo.write_config(
        &server.url("/feed.xml"),
        "url = \"https://reads.example.com\"",
    );
    repo.aggr()
        .arg("check")
        .assert()
        .success()
        .stdout(predicate::str::contains("ok     demo"))
        .stdout(predicate::str::contains("2 item(s)"));
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
         [[sources]]\ninclude = \"./aggr-*.toml\"\ncategory = \"ai\"\n\
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
