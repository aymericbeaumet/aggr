use std::fs;

use chrono::TimeZone;
use httpmock::prelude::*;
use url::Url;

use super::*;
use crate::model::FrontMatter;
use crate::store::NewItem;

pub(crate) fn source() -> Source {
    Source {
        slug: "blog".into(),
        name: None,
        category: None,
        labels: vec![],
        identity: "blog".into(),
        public_url: Some("https://blog.example/feed".into()),
        persist_endpoint: true,
        headers: vec![],
        html: true,
        content: ContentMode::Heavy,
        previews: crate::config::PreviewPolicy::Disabled,
        images: crate::config::ImagePolicy::Original,
        documents: crate::config::DocumentPolicy::Original,
        limits: crate::config::Limits::default(),
        engine: crate::config::Engine::Feed {
            url: Url::parse("https://blog.example/feed").unwrap(),
        },
    }
}

#[tokio::test]
async fn mirrored_articles_never_download_the_original_even_in_heavy_mode() {
    let server = MockServer::start();
    let original = server.mock(|when, then| {
        when.path("/article");
        then.status(500);
    });
    let mut source = source();
    source.engine = Engine::Aggr {
        url: Url::parse("https://github.com/example/archive.git").unwrap(),
        branch: "aggr".into(),
        sources: Vec::new(),
    };
    let raw = RawItem {
        link: server.url("/article"),
        content_html: Some("<p>Already archived</p>".into()),
        ..Default::default()
    };
    let cache = tempfile::tempdir().unwrap();
    let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
    let (mirrored, kind) = heavy_content(
        &raw,
        &source,
        &client,
        cache.path(),
        &ArticleFailures::default(),
    )
    .await;
    assert_eq!(mirrored, raw);
    assert_eq!(kind, ContentKind::Feed);
    original.assert_calls(0);
}

#[tokio::test]
async fn changed_mirror_hydrates_only_new_writable_companions() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let calls = AtomicUsize::new(0);
    let mut known = RawItem {
        title: "Known with corrupt companion".into(),
        link: "https://example.com/known".into(),
        ..Default::default()
    };
    crate::sources::aggr::attach_companion_locator(
        &mut known,
        Path::new("/instrumented-mirror"),
        "items/source/known-corrupt",
    )
    .unwrap();
    let known =
        hydrate_new_mirror_companions_with(known, true, false, true, true, false, |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            panic!("known mirror companion must not be read")
        })
        .await;
    assert!(known.images.is_empty());

    let mut new = RawItem {
        title: "New".into(),
        link: "https://example.com/new".into(),
        ..Default::default()
    };
    crate::sources::aggr::attach_companion_locator(
        &mut new,
        Path::new("/instrumented-mirror"),
        "items/source/new",
    )
    .unwrap();
    let new =
        hydrate_new_mirror_companions_with(new, false, false, true, true, false, |mut raw| async {
            calls.fetch_add(1, Ordering::SeqCst);
            crate::sources::aggr::discard_companion_locator(&mut raw);
            raw
        })
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(new.images.is_empty());

    let dry_run = RawItem {
        title: "Dry run".into(),
        link: "https://example.com/dry".into(),
        ..Default::default()
    };
    let _ =
        hydrate_new_mirror_companions_with(dry_run, false, true, true, true, false, |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            panic!("dry-run mirror companion must not be read")
        })
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn mirrored_companions_respect_source_media_options() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let calls = AtomicUsize::new(0);
    let mirrored = || {
        let mut raw = RawItem {
            title: "Mirrored".into(),
            link: "https://example.com/mirrored".into(),
            ..Default::default()
        };
        crate::sources::aggr::attach_companion_locator(
            &mut raw,
            Path::new("/instrumented-mirror"),
            "items/source/mirrored",
        )
        .unwrap();
        raw
    };
    let mut preview_bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(40, 20)
        .write_to(&mut preview_bytes, image::ImageFormat::Png)
        .unwrap();
    let preview = crate::preview::thumbnail(&preview_bytes.into_inner(), None).unwrap();
    let image = crate::media::Asset {
        source_url: "https://example.com/image.png".into(),
        source_hash: "source".into(),
        alt: None,
        master_bytes: vec![1],
        master_extension: "png",
        master_hash: "master".into(),
        width: 40,
        height: 20,
        dominant_color: "#000000".into(),
        placeholder: crate::media::placeholder::from_bytes(&preview.bytes).unwrap(),
        renditions: Vec::new(),
    };
    let disabled = hydrate_new_mirror_companions_with(
        mirrored(),
        false,
        false,
        false,
        false,
        false,
        |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            panic!("disabled mirror companions must not be read")
        },
    )
    .await;
    assert!(disabled.preview.is_none());
    assert!(disabled.images.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let without_previews = hydrate_new_mirror_companions_with(
        mirrored(),
        false,
        false,
        false,
        true,
        false,
        |mut raw| async {
            calls.fetch_add(1, Ordering::SeqCst);
            raw.preview = Some(preview.clone());
            raw.images.push(image.clone());
            raw
        },
    )
    .await;
    assert!(without_previews.preview.is_none());
    assert_eq!(without_previews.images.len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let without_images = hydrate_new_mirror_companions_with(
        mirrored(),
        false,
        false,
        true,
        false,
        false,
        |mut raw| async {
            calls.fetch_add(1, Ordering::SeqCst);
            raw.preview = Some(preview);
            raw.images.push(image);
            raw
        },
    )
    .await;
    assert!(without_images.preview.is_some());
    assert!(without_images.images.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

pub(super) fn options() -> Options {
    Options {
        archived_links: Arc::default(),
        existing_paths: Arc::default(),
        dry_run: false,
        refresh: false,
        backfill_media: false,
        html_max_bytes: 1000,
        article_concurrency: 4,
        preparation_limit: Arc::new(Semaphore::new(preparation_slots(
            StatePolicy::PersistentBranch,
        ))),
        persist_limit: Arc::new(Semaphore::new(preparation_slots(
            StatePolicy::PersistentBranch,
        ))),
        recording_limit: Arc::new(Semaphore::new(RECORDING_PROBE_SLOTS)),
        preview_fetcher: Arc::new(preview::Fetcher::new().unwrap()),
        media_fetcher: Arc::new(
            media::Fetcher::new(
                &crate::config::FetchConfig::default(),
                media::MediaLimits::default(),
            )
            .unwrap(),
        ),
        now: Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap(),
    }
}

#[tokio::test]
async fn recording_probes_and_article_preparation_never_wait_for_each_other() {
    crate::http::install_crypto_provider();
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.path("/feed");
            then.status(304);
        })
        .await;
    let episode_page = server
        .mock_async(|when, then| {
            when.method(GET).path("/episode");
            then.status(200)
                .header("content-type", "text/html")
                .body(format!(
                    r#"<html><head><script type="application/ld+json">{{"@type":"AudioObject","contentUrl":"{}","duration":"PT27M51S"}}</script></head><body><p>Show notes</p></body></html>"#,
                    server.url("/episode.mp3")
                ));
        })
        .await;
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(root.path()));
    let mut front = FrontMatter {
        title: "Episode".into(),
        source: "blog".into(),
        link: server.url("/episode"),
        first_seen: "2026-07-16T12:00:00Z".parse().unwrap(),
        content: ContentKind::Extracted,
        ..Default::default()
    };
    front
        .extra
        .insert("audio_url".into(), server.url("/episode.mp3").into());
    store
        .write_item(NewItem {
            dir: "items/blog",
            stem: "episode",
            front: &front,
            body: "Show notes\n",
            html: None,
            preview: None,
            images: &[],
        })
        .unwrap();
    let (_, archive) = index_archive(store.items().unwrap());
    assert_eq!(archive.recordings.get("blog").map(Vec::len), Some(1));
    let test_options = Options {
        existing_paths: Arc::new(OnceCell::new_with(Some(archive))),
        ..options()
    };
    let configured = Source {
        engine: Engine::Feed {
            url: Url::parse(&server.url("/feed")).unwrap(),
        },
        ..source()
    };
    let client = http::Client::new(&crate::config::FetchConfig {
        retries: 0,
        ..Default::default()
    })
    .unwrap();

    // Every preparation slot is busy with slow articles from other sources: the probe for
    // the archived episode must still run and record its duration.
    let preparations = (0..preparation_slots(StatePolicy::PersistentBranch))
        .map(|_| {
            test_options
                .preparation_limit
                .clone()
                .try_acquire_owned()
                .unwrap()
        })
        .collect::<Vec<_>>();
    let report = tokio::time::timeout(
        Duration::from_secs(10),
        fetch_one(
            &configured,
            FetchOneContext {
                store: &store,
                store_root: root.path(),
                client: &client,
                cache_dir: cache.path(),
                options: &test_options,
                article_failures: &ArticleFailures::default(),
                state_policy: StatePolicy::DevCache,
            },
        ),
    )
    .await
    .expect("the recording probe waited for an article preparation slot")
    .unwrap();
    assert_eq!(report.added, 1);
    episode_page.assert_calls_async(1).await;
    assert_eq!(
        store
            .read_item("items/blog/episode")
            .unwrap()
            .front
            .extra
            .get("duration_seconds")
            .and_then(serde_yaml_ng::Value::as_u64),
        Some(1671)
    );
    drop(preparations);

    // And the other way round: probes stuck in slow requests never stall preparation.
    let probes = (0..RECORDING_PROBE_SLOTS)
        .map(|_| {
            test_options
                .recording_limit
                .clone()
                .try_acquire_owned()
                .unwrap()
        })
        .collect::<Vec<_>>();
    let raw = RawItem {
        title: "Ready".into(),
        link: server.url("/ready"),
        content_html: Some("<p>Body</p>".into()),
        ..Default::default()
    };
    let (_, planned) = tokio::time::timeout(
        Duration::from_secs(10),
        prepare_item(raw, &configured, &test_options, ContentKind::Extracted),
    )
    .await
    .expect("article preparation waited for recording probes")
    .unwrap();
    assert_eq!(planned.body.trim(), "Body");
    drop(probes);
}

#[test]
fn one_parse_yields_every_page_derived_fact() {
    let page_url = Url::parse("https://example.test/episodes/42").unwrap();
    let audio = "https://cdn.test/episode.mp3";
    let page = format!(
        r#"<html><head>
<link rel="alternate" type="application/activity+json" href="/objects/42">
<meta property="og:image" content="/cover.png">
<script type="application/ld+json">{{"@type":"AudioObject","contentUrl":"{audio}","duration":"PT27M51S"}}</script>
</head><body><main><canvas id="surface"></canvas><article><img data-src="/lazy.png"></article></main>
<aside><label>Shape<select></select></label><input type="range"><button>Reset</button></aside>
<script src="/vendor/three.js"></script></body></html>"#
    );
    let analysis = analyze_page(page.clone(), &page_url, &page_url, Some(audio), true);
    assert_eq!(analysis.duration, Some(1671));
    assert_eq!(analysis.publisher, None);
    assert!(analysis.interactive);
    assert_eq!(
        preview::ordered_article_candidates(&[], analysis.candidates, None)
            .iter()
            .map(|candidate| candidate.url.as_str())
            .collect::<Vec<_>>(),
        [
            "https://example.test/cover.png",
            "https://example.test/lazy.png"
        ]
    );
    assert_eq!(
        analysis
            .activity_alternates
            .iter()
            .map(Url::as_str)
            .collect::<Vec<_>>(),
        ["https://example.test/objects/42"]
    );
    assert_eq!(analysis.page, page);

    let plain = analyze_page(
        "<p>Nothing to see</p>".into(),
        &page_url,
        &page_url,
        None,
        false,
    );
    assert_eq!(plain.duration, None);
    assert_eq!(plain.publisher, None);
    assert!(!plain.interactive);
    assert!(preview::ordered_article_candidates(&[], plain.candidates, None).is_empty());
    assert!(plain.activity_alternates.is_empty());
}

#[test]
fn a_watch_page_names_the_channel_its_own_url_cannot() {
    let page = r#"<script>var ytInitialPlayerResponse = {"videoDetails":{"videoId":"abc","channelId":"UC123"},"microformat":{"playerMicroformatRenderer":{"ownerProfileUrl":"http://www.youtube.com/@Veritasium","lengthSeconds":"600"}}};</script>"#;
    let watch = Url::parse("https://www.youtube.com/watch?v=abc").unwrap();
    let analysis = analyze_page(page.into(), &watch, &watch, None, false);
    assert_eq!(
        analysis.publisher.as_deref(),
        Some("https://www.youtube.com/@Veritasium")
    );
    assert_eq!(analysis.duration, Some(600));

    // A URL that already names its account has nothing to ask the page for.
    let profile = Url::parse("https://www.youtube.com/@Veritasium/videos").unwrap();
    assert_eq!(
        analyze_page(page.into(), &profile, &profile, None, false).publisher,
        None
    );
}

#[tokio::test]
async fn pages_without_activitypub_markers_are_never_probed_for_threads() {
    let server = MockServer::start_async().await;
    // Registered first: any request negotiating an ActivityPub representation lands here.
    let discovery = server
        .mock_async(|when, then| {
            when.header_matches("accept", ".*activity\\+json.*");
            then.status(500);
        })
        .await;
    let page = server
        .mock_async(|when, then| {
            when.method(GET).path("/post");
            then.status(200).header("content-type", "text/html").body(
                r#"<html><head><link rel="alternate" type="application/rss+xml" href="/feed.xml"></head><body><article><h1>Plain</h1><p>An ordinary article links to <a href="/objects/thread">a thread</a> without advertising any social representation of itself.</p><p>Its second paragraph keeps the extraction readable and well above the minimum.</p></article></body></html>"#,
            );
        })
        .await;
    let raw = RawItem {
        title: "Plain".into(),
        link: server.url("/post"),
        content_html: Some("<p>short feed excerpt</p>".into()),
        ..Default::default()
    };
    let client = http::Client::new(&crate::config::FetchConfig {
        retries: 0,
        ..Default::default()
    })
    .unwrap();
    let cache = tempfile::tempdir().unwrap();

    let (extracted, kind) = heavy_content(
        &raw,
        &source(),
        &client,
        cache.path(),
        &ArticleFailures::default(),
    )
    .await;

    assert_eq!(kind, ContentKind::Extracted);
    assert!(extracted.content_html.unwrap().contains("ordinary article"));
    page.assert_calls_async(1).await;
    discovery.assert_calls_async(0).await;
}

#[tokio::test]
async fn slow_first_article_does_not_block_later_downloads_or_change_filenames() {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

    crate::http::install_crypto_provider();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let release_first = Arc::new(Semaphore::new(0));
    let (requests, mut received) = tokio::sync::mpsc::unbounded_channel();
    let gate = release_first.clone();
    let articles = tokio::spawn(async move {
        let mut responses = JoinSet::new();
        for _ in 0..3 {
            let (socket, _) = listener.accept().await.unwrap();
            let requests = requests.clone();
            let gate = gate.clone();
            responses.spawn(async move {
                let mut socket = BufReader::new(socket);
                let mut line = String::new();
                socket.read_line(&mut line).await.unwrap();
                let path = line.split_whitespace().nth(1).unwrap().to_string();
                loop {
                    line.clear();
                    socket.read_line(&mut line).await.unwrap();
                    if line == "\r\n" { break; }
                }
                requests.send(path.clone()).unwrap();
                if path == "/first" {
                    let _permit = gate.acquire().await.unwrap();
                }
                let body = format!("<article><h1>Same</h1><p>This is the complete article for {path}, with enough original readable text to preserve its contents.</p><p>The next paragraph supplies further relevant detail for this article.</p></article>");
                socket.get_mut().write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            });
        }
        while responses.join_next().await.is_some() {}
    });
    let server = MockServer::start_async().await;
    server.mock_async(|when, then| {
        when.path("/feed");
        then.status(200).header("content-type", "application/feed+json").json_body(serde_json::json!({
            "version": "https://jsonfeed.org/version/1.1", "title": "Slow article test",
            "items": ([("third", 3), ("second", 2), ("first", 1)].into_iter().map(|(path, hour)| serde_json::json!({
                "id": path, "title": "Same", "url": format!("{base}/{path}"),
                "date_published": format!("2026-09-01T0{hour}:00:00Z"), "content_text": "Excerpt"
            })).collect::<Vec<_>>())
        }));
    }).await;
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(root.path()));
    let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
    let configured = Source {
        images: crate::config::ImagePolicy::Remote,
        documents: crate::config::DocumentPolicy::Original,
        engine: Engine::Feed {
            url: Url::parse(&server.url("/feed")).unwrap(),
        },
        ..source()
    };
    let test_options = Options {
        article_concurrency: 2,
        ..options()
    };
    let failures = ArticleFailures::default();
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(
            fetch_one(
                &configured,
                FetchOneContext {
                    store: &store,
                    store_root: root.path(),
                    client: &client,
                    cache_dir: cache.path(),
                    options: &test_options,
                    article_failures: &failures,
                    state_policy: StatePolicy::PersistentBranch,
                }
            ),
            async {
                while let Some(path) = received.recv().await {
                    if path == "/third" {
                        release_first.add_permits(1);
                        return;
                    }
                }
                panic!("third article was never requested");
            }
        )
    })
    .await;
    articles.abort();
    let (report, ()) = result.expect("third article must start while the first is still blocked");
    assert_eq!(report.unwrap().added, 3);
    let stored = store.items().unwrap();
    let first = stored
        .iter()
        .find(|item| item.front.link.ends_with("/first"))
        .unwrap();
    assert!(first.path.ends_with("/2026-09-01-same"), "{}", first.path);
    assert_eq!(
        stored
            .iter()
            .map(|item| &item.path)
            .collect::<HashSet<_>>()
            .len(),
        3
    );
}

#[tokio::test]
async fn light_youtube_videos_keep_feed_content_without_waiting_for_a_page_request() {
    use futures_util::FutureExt as _;

    crate::http::install_crypto_provider();
    let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
    let cache = tempfile::tempdir().unwrap();
    for link in [
        "https://www.youtube.com/watch?v=video",
        "https://youtu.be/video",
    ] {
        let raw = RawItem {
            title: "Video".into(),
            link: link.into(),
            content_html: Some("<p>Video description</p>".into()),
            ..Default::default()
        };
        let mut light = source();
        light.content = ContentMode::Light;
        let (kept, kind) = heavy_content(
            &raw,
            &light,
            &client,
            cache.path(),
            &ArticleFailures::default(),
        )
        .now_or_never()
        .expect("YouTube metadata must not await an article request");
        assert_eq!(kind, ContentKind::Feed);
        assert_eq!(kept.content_html, raw.content_html);
    }
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn binary_articles_keep_feed_content_without_a_duplicate_html_download() {
    use futures_util::FutureExt as _;
    crate::http::install_crypto_provider();
    let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let mut heavy = source();
    heavy.content = ContentMode::Heavy;
    for link in [
        "https://example.com/paper.PDF?download=1",
        "https://example.com/paper%2Epdf",
        "https://example.com/download?filename=Paper%20One.PDF",
        "https://example.com/photo.png",
        "https://example.com/photo.JPEG#full",
    ] {
        let raw = RawItem {
            title: "Retained article".into(),
            link: link.into(),
            content_html: Some("<p>Existing feed description</p>".into()),
            ..Default::default()
        };
        let (kept, kind) = heavy_content(
            &raw,
            &heavy,
            &client,
            cache.path(),
            &ArticleFailures::default(),
        )
        .now_or_never()
        .expect("binary article previews must not trigger an earlier HTML download");
        assert_eq!(kind, ContentKind::Feed);
        assert_eq!(kept.content_html, raw.content_html);
        assert_eq!(kept.link, raw.link);
    }
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn qwen_full_api_articles_do_not_request_javascript_shells() {
    use futures_util::FutureExt as _;
    crate::http::install_crypto_provider();
    let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let mut source = source();
    source.content = ContentMode::Heavy;
    source.engine = Engine::Feed {
        url: Url::parse("https://qwen.ai/blog").unwrap(),
    };
    let raw = RawItem {
        link: "https://qwen.ai/blog?id=release".into(),
        content_html: Some("<p>The complete API article.</p>".into()),
        ..Default::default()
    };
    let (kept, kind) = heavy_content(
        &raw,
        &source,
        &client,
        cache.path(),
        &ArticleFailures::default(),
    )
    .now_or_never()
    .expect("full Qwen API content must not await a page request");
    assert_eq!(kind, ContentKind::Feed);
    assert_eq!(kept.content_html, raw.content_html);
    assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn heavy_downloads_the_article_while_light_keeps_feed_content() {
    let server = MockServer::start_async().await;
    let article = server
        .mock_async(|when, then| {
            when.method(GET).path("/post");
            then.status(200).header("etag", "\"article-v1\"").body(
                "<html><head><title>Post</title><meta property='article:tag' content='MCP'><meta property='article:tag' content='AI'></head><article><h1>Post</h1><p>The complete original article has substantially more useful text than its feed excerpt.</p><p>This second paragraph makes it readable.</p></article></html>",
            );
        })
        .await;
    let raw = RawItem {
        title: "Post".into(),
        labels: vec!["existing".into(), "MCP".into()],
        link: server.url("/post"),
        content_html: Some("<p>short feed excerpt</p>".into()),
        ..Default::default()
    };
    let client = http::Client::new(&crate::config::FetchConfig {
        retries: 0,
        ..Default::default()
    })
    .unwrap();
    let cache = tempfile::tempdir().unwrap();

    let failures = ArticleFailures::default();
    let (heavy, kind) = heavy_content(&raw, &source(), &client, cache.path(), &failures).await;
    assert_eq!(kind, ContentKind::Extracted);
    assert_eq!(heavy.labels, ["ai", "existing", "mcp"]);
    assert!(
        heavy
            .content_html
            .unwrap()
            .contains("complete original article")
    );
    article.assert_calls_async(1).await;
    article.delete_async().await;
    let not_modified = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/post")
                .header("if-none-match", "\"article-v1\"");
            then.status(304);
        })
        .await;
    let (cached, kind) = heavy_content(&raw, &source(), &client, cache.path(), &failures).await;
    assert_eq!(kind, ContentKind::Extracted);
    assert_eq!(cached.labels, ["ai", "existing", "mcp"]);
    assert!(
        cached
            .content_html
            .unwrap()
            .contains("complete original article")
    );
    not_modified.assert_calls_async(1).await;

    let light = Source {
        content: ContentMode::Light,
        ..source()
    };
    let (unchanged, kind) = heavy_content(&raw, &light, &client, cache.path(), &failures).await;
    assert_eq!(kind, ContentKind::Feed);
    assert_eq!(unchanged.content_html, raw.content_html);
    not_modified.assert_calls_async(1).await;
}

#[tokio::test]
async fn heavy_reads_a_script_shell_article_from_its_module() {
    // z.ai: an empty `<div id="root">` and one module script holding the compiled MDX post.
    let server = MockServer::start_async().await;
    let shell = r#"<!doctype html><html lang="en"><head><meta charset="UTF-8"><meta property="og:title" content="How GLM Built Its Own Inference Infrastructure"><script type="module" crossorigin src="/blog/assets/glm-built-its-inference-infrastructure-qEyLd6aI.js"></script><link rel="modulepreload" crossorigin href="/blog/assets/src-GO5ZQO2t.js"><link rel="modulepreload" href="https://cdn.other/vendor.js"></head><body><div id="root"></div></body></html>"#;
    server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/blog/glm-built-its-inference-infrastructure");
            then.status(200)
                .header("content-type", "text/html")
                .body(shell);
        })
        .await;
    let module = r#"import{_ as e,d as t,f as n,i as r,l as i,r as a,t as o,u as s}from"./src-GO5ZQO2t.js";var c=t(),l=e(n(),1),u=i();function d(e){let t={p:`p`,...e.components};return(0,u.jsxs)(u.Fragment,{children:[(0,u.jsx)(t.p,{children:`As we develop GLM, the model sometimes exhibits capabilities that surprise us, and even unsettle us.`}),`
`,(0,u.jsx)(t.p,{children:`In October 2025, we began researching how to strengthen its cybersecurity capabilities. Our reasoning at the time was straightforward: cybersecurity is a natural extension of coding.`}),`
`,(0,u.jsx)(t.h1,{children:`Driving Infra Agent Optimization of Inference Systems with Dense Feedback`}),`
`,(0,u.jsx)(t.p,{children:`Taking a model from its first successful run on new hardware to a high-performance production launch is a long journey of tuning, profiling, and rewriting the parts that turn out to be slow.`}),`
`,(0,u.jsxs)(t.p,{children:[`The numerical accuracy fixes have been merged upstream. See `,(0,u.jsx)(t.a,{href:`https://github.com/fla-org/flash-linear-attention/pull/1180`,children:`PR #1180`}),` for details.`]}),`
`,(0,u.jsx)(t.p,{children:`Of course, we have not yet reached recursive self-improvement. Choosing objectives, setting boundaries, and assessing risk remain human responsibilities for a long time to come.`})]})}(0,c.createRoot)(document.getElementById(`root`)).render((0,u.jsx)(l.StrictMode,{children:(0,u.jsx)(a,{title:`Toward Recursive Self-Improvement`,children:(0,u.jsx)(r,{en:d,components:o})})}));"#;
    let entry = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/blog/assets/glm-built-its-inference-infrastructure-qEyLd6aI.js");
            then.status(200)
                .header("content-type", "text/javascript")
                .body(module);
        })
        .await;
    let preload = server
        .mock_async(|when, then| {
            when.method(GET).path("/blog/assets/src-GO5ZQO2t.js");
            then.status(200)
                .header("content-type", "text/javascript")
                .body("export const d=()=>1;");
        })
        .await;
    let raw = RawItem {
        title: "GLM Built Its Own Inference Infrastructure".into(),
        link: server.url("/blog/glm-built-its-inference-infrastructure"),
        content_html: Some("<p>Article URL: https://z.ai/blog/x Points: 141</p>".into()),
        ..Default::default()
    };
    let client = http::Client::new(&crate::config::FetchConfig {
        retries: 0,
        ..Default::default()
    })
    .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let failures = ArticleFailures::default();
    let (heavy, kind) = heavy_content(&raw, &source(), &client, cache.path(), &failures).await;
    assert_eq!(kind, ContentKind::Extracted, "{:?}", heavy.content_html);
    let html = heavy.content_html.unwrap();
    assert!(html.contains("As we develop GLM"), "{html}");
    assert!(html.contains("Driving Infra Agent Optimization"), "{html}");
    assert!(html.contains("href=\"https://github.com/fla-org/flash-linear-attention/pull/1180\""));
    assert!(!html.contains("StrictMode") && !html.contains("Toward Recursive"));
    entry.assert_calls_async(1).await;
    preload.assert_calls_async(1).await;
    assert!(!failures.blocked(&url::Url::parse(&raw.link).unwrap()));
    // The extraction is cached against the shell: a second pass fetches no module.
    let (again, kind) = heavy_content(&raw, &source(), &client, cache.path(), &failures).await;
    assert_eq!(kind, ContentKind::Extracted);
    assert!(again.content_html.unwrap().contains("As we develop GLM"));
    entry.assert_calls_async(1).await;
}

#[tokio::test]
async fn heavy_keeps_the_entry_when_the_readable_region_is_not_the_item() {
    // A short release note whose page also carries a long sidebar: Readability prefers the
    // sidebar, the feed summary is the note.
    let server = MockServer::start_async().await;
    let promo = "<p>Sponsor me for ten dollars a month and get a curated email digest of the month's most important developments in this field.</p>".repeat(6);
    let body = format!(
        r#"<html><head><title>datasette 0.65.5</title></head><body><div id="primary"><div class="beat"><span>Release</span> <span>datasette 0.65.5</span> <span>An open source multi-tool for exploring and publishing data</span><div class="beat-note"><p>Security fix for an issue where a trailing newline in a requested table name could bypass table permissions and expose private rows, reported by someone in GHSA-h547-rmjf-5m2m.</p></div></div></div><div id="secondary"><div class="metabox"><p>This is a beat by the author, posted on 16th September 2026.</p><section><h3>Monthly briefing</h3>{promo}</section></div></div></body></html>"#
    );
    server
        .mock_async(|when, then| {
            when.method(GET).path("/2026/Sep/16/datasette-2/");
            then.status(200)
                .header("content-type", "text/html")
                .body(body);
        })
        .await;
    let raw = RawItem {
        title: "datasette 0.65.5".into(),
        link: server.url("/2026/Sep/16/datasette-2/"),
        summary: Some("Release: datasette 0.65.5 Security fix for an issue where a trailing newline in a requested table name could bypass table permissions and expose private rows, reported by someone in GHSA-h547-rmjf-5m2m. Tags: security, datasette".into()),
        ..Default::default()
    };
    let client = http::Client::new(&crate::config::FetchConfig {
        retries: 0,
        ..Default::default()
    })
    .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let failures = ArticleFailures::default();
    let (kept, kind) = heavy_content(&raw, &source(), &client, cache.path(), &failures).await;
    assert_eq!(kind, ContentKind::None, "{:?}", kept.content_html);
    assert_eq!(kept.content_html, None);
    assert!(!failures.blocked(&url::Url::parse(&raw.link).unwrap()));
    let planned = super::plan::plan(&kept, &source(), &options(), kind);
    assert!(
        planned.body.starts_with("Release: datasette 0.65.5"),
        "{}",
        planned.body
    );
    assert!(!planned.body.contains("Monthly briefing"));
}

#[tokio::test]
async fn heavy_prefers_a_discovered_public_activitypub_self_thread() {
    let server = MockServer::start_async().await;
    let activity_url = server.url("/users/alice/statuses/100");
    let actor_url = server.url("/users/alice");
    let child_url = server.url("/users/alice/statuses/101");
    let page = server
        .mock_async(|when, then| {
            when.method(GET).path("/@alice/100");
            then.status(200).header("content-type", "text/html").body(format!(
                r#"<html><head><link rel="alternate" type="application/activity+json" href="{activity_url}"></head><body><article><p>This ordinary fallback article contains enough readable text for extraction if thread expansion does not work.</p><p>It should not replace the discovered thread.</p></article></body></html>"#
            ));
        })
        .await;
    let activity = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/users/alice/statuses/100")
                .header_matches("accept", ".*application/activity\\+json.*");
            then.status(200)
                .header("content-type", "application/activity+json")
                .json_body(serde_json::json!({
                    "id": activity_url,
                    "type": "Note",
                    "attributedTo": actor_url,
                    "to": ["https://www.w3.org/ns/activitystreams#Public"],
                    "content": "<p>Thread opening</p>",
                    "replies": {"items": [{
                        "id": child_url,
                        "type": "Note",
                        "attributedTo": actor_url,
                        "inReplyTo": activity_url,
                        "to": ["https://www.w3.org/ns/activitystreams#Public"],
                        "content": "<p>Thread continuation</p>",
                        "replies": {"items": []}
                    }]}
                }));
        })
        .await;
    let raw = RawItem {
        title: "Thread".into(),
        link: server.url("/@alice/100"),
        content_html: Some("<p>short feed excerpt</p>".into()),
        ..Default::default()
    };
    let client = http::Client::new(&crate::config::FetchConfig {
        retries: 0,
        ..Default::default()
    })
    .unwrap();
    let cache = tempfile::tempdir().unwrap();

    let (expanded, kind) = heavy_content(
        &raw,
        &source(),
        &client,
        cache.path(),
        &ArticleFailures::default(),
    )
    .await;

    assert_eq!(kind, ContentKind::Extracted);
    let html = expanded.content_html.unwrap();
    assert!(html.contains("Thread opening"), "{html}");
    assert!(html.contains("Thread continuation"), "{html}");
    assert!(!html.contains("ordinary fallback"), "{html}");
    page.assert_calls_async(1).await;
    activity.assert_calls_async(1).await;
}

#[tokio::test]
async fn activitypub_failure_nonfatally_uses_normal_article_extraction() {
    let server = MockServer::start_async().await;
    let activity_url = server.url("/activity/100");
    let page = server
        .mock_async(|when, then| {
            when.method(GET).path("/post");
            then.status(200).header("content-type", "text/html").body(format!(
                r#"<html><head><link rel="alternate" type="application/activity+json" href="{activity_url}"></head><body><article><h1>Fallback</h1><p>The complete ordinary article remains available when its advertised social representation cannot be loaded.</p><p>This second paragraph keeps the document readable.</p></article></body></html>"#
            ));
        })
        .await;
    let failed_activity = server
        .mock_async(|when, then| {
            when.method(GET).path("/activity/100");
            then.status(500);
        })
        .await;
    let raw = RawItem {
        title: "Fallback".into(),
        link: server.url("/post"),
        content_html: Some("<p>short feed excerpt</p>".into()),
        ..Default::default()
    };
    let client = http::Client::new(&crate::config::FetchConfig {
        retries: 0,
        ..Default::default()
    })
    .unwrap();
    let cache = tempfile::tempdir().unwrap();

    let (fallback, kind) = heavy_content(
        &raw,
        &source(),
        &client,
        cache.path(),
        &ArticleFailures::default(),
    )
    .await;

    assert_eq!(kind, ContentKind::Extracted);
    assert!(
        fallback
            .content_html
            .unwrap()
            .contains("complete ordinary article")
    );
    page.assert_calls_async(1).await;
    failed_activity.assert_calls_async(1).await;
}

#[tokio::test]
async fn heavy_stops_retrying_an_article_that_denies_requests() {
    let server = MockServer::start_async().await;
    let denied = server
        .mock_async(|when, then| {
            when.method(GET).path("/post");
            then.status(403);
        })
        .await;
    let raw = RawItem {
        title: "Post".into(),
        link: server.url("/post"),
        content_html: Some("<p>feed copy</p>".into()),
        ..Default::default()
    };
    let client = http::Client::new(&crate::config::FetchConfig {
        retries: 0,
        ..Default::default()
    })
    .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let failures = ArticleFailures::default();

    for _ in 0..3 {
        let (item, kind) = heavy_content(&raw, &source(), &client, cache.path(), &failures).await;
        assert_eq!(kind, ContentKind::Feed);
        assert_eq!(item.content_html, raw.content_html);
    }
    denied.assert_calls_async(1).await;
}

#[tokio::test]
async fn article_denials_preserve_feed_items_and_allow_other_pages_on_the_origin() {
    for status in [401, 403] {
        let server = MockServer::start_async().await;
        let denied = server
            .mock_async(|when, then| {
                when.method(GET).path("/private");
                then.status(status);
            })
            .await;
        let allowed = server.mock_async(|when, then| {
            when.method(GET).path("/public");
            then.status(200).header("content-type", "text/html").body(
                "<article><h1>Public article</h1><p>This complete public article remains accessible when another page on the same site requires authorization.</p><p>The feed must continue enriching its other items independently.</p></article>",
            );
        }).await;
        let raw = |path: &str| RawItem {
            title: "Article".into(),
            link: server.url(path),
            content_html: Some("<p>feed copy</p>".into()),
            ..Default::default()
        };
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let failures = ArticleFailures::default();
        let private = raw("/private");
        let (item, kind) =
            heavy_content(&private, &source(), &client, cache.path(), &failures).await;
        assert_eq!(kind, ContentKind::Feed);
        assert_eq!(item.link, private.link);
        assert_eq!(item.content_html, private.content_html);
        let (item, kind) =
            heavy_content(&raw("/public"), &source(), &client, cache.path(), &failures).await;
        assert_eq!(kind, ContentKind::Extracted);
        assert!(
            item.content_html
                .unwrap()
                .contains("complete public article")
        );
        denied.assert_calls_async(1).await;
        allowed.assert_calls_async(1).await;
    }
}

#[tokio::test]
async fn article_rate_limits_preserve_items_and_pause_only_the_affected_origin() {
    let server = MockServer::start_async().await;
    let limited = server
        .mock_async(|when, then| {
            when.method(GET);
            then.status(429);
        })
        .await;
    let client = http::Client::new(&crate::config::FetchConfig {
        retries: 0,
        ..Default::default()
    })
    .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let failures = ArticleFailures::default();
    for path in ["/first", "/second"] {
        let raw = RawItem {
            title: "Article".into(),
            link: server.url(path),
            content_html: Some("<p>feed copy</p>".into()),
            ..Default::default()
        };
        let (item, kind) = heavy_content(&raw, &source(), &client, cache.path(), &failures).await;
        assert_eq!(kind, ContentKind::Feed);
        assert_eq!(item.content_html, raw.content_html);
        assert_eq!(item.link, raw.link);
    }
    limited.assert_calls_async(1).await;
    assert!(!failures.blocked(&Url::parse("https://other.example/post").unwrap()));
}

#[tokio::test]
async fn article_denials_do_not_cross_source_credentials_on_one_origin() {
    let server = MockServer::start_async().await;
    let denied = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/private")
                .header("authorization", "Bearer rejected");
            then.status(403);
        })
        .await;
    let allowed = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/public")
                .header("authorization", "Bearer accepted");
            then.status(200).header("content-type", "text/html").body(
                "<article><h1>Allowed</h1><p>This independently authorized source keeps its complete readable article.</p><p>It must not inherit another source's denial.</p></article>",
            );
        })
        .await;
    let make_source = |slug: &str, token: &str| {
        let mut source = source();
        source.slug = slug.into();
        source.headers = vec![("Authorization".into(), format!("Bearer {token}"))];
        source.engine = Engine::Feed {
            url: Url::parse(&server.url("/feed")).unwrap(),
        };
        source
    };
    let rejected = make_source("rejected", "rejected");
    let accepted = make_source("accepted", "accepted");
    let raw = |path: &str| RawItem {
        title: "Article".into(),
        link: server.url(path),
        content_html: Some("<p>feed copy</p>".into()),
        ..Default::default()
    };
    let client = http::Client::new(&crate::config::FetchConfig {
        retries: 0,
        ..Default::default()
    })
    .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let rejected_failures = ArticleFailures::default();
    let accepted_failures = ArticleFailures::default();

    let (_, rejected_kind) = heavy_content(
        &raw("/private"),
        &rejected,
        &client,
        cache.path(),
        &rejected_failures,
    )
    .await;
    let (article, accepted_kind) = heavy_content(
        &raw("/public"),
        &accepted,
        &client,
        cache.path(),
        &accepted_failures,
    )
    .await;
    assert_eq!(rejected_kind, ContentKind::Feed);
    assert_eq!(accepted_kind, ContentKind::Extracted);
    assert!(
        article
            .content_html
            .unwrap()
            .contains("independently authorized")
    );
    denied.assert_calls_async(1).await;
    allowed.assert_calls_async(1).await;
}

#[test]
fn binary_links_are_pdfs_and_image_files_whatever_their_case() {
    for link in [
        "https://example.com/paper.pdf",
        "https://example.com/paper.PDF?download=1",
        "https://example.com/paper%2Epdf",
        "https://example.com/download?filename=Paper%20One.PDF",
        "https://example.com/photo.png",
        "https://example.com/photo.JPEG#full",
        "https://example.com/gallery/pic.jpg",
        "https://example.com/pic.webp",
        "https://example.com/anim.gif",
        "https://example.com/pic.avif",
    ] {
        assert!(is_binary_link(&Url::parse(link).unwrap()), "{link}");
    }
    for link in [
        "https://example.com/article",
        "https://example.com/post.html",
        "https://example.com/png/gallery",
        "https://example.com/download?filename=photo.png",
        "https://example.com/video.mp4",
    ] {
        assert!(!is_binary_link(&Url::parse(link).unwrap()), "{link}");
    }
}

#[tokio::test]
async fn mirrored_document_is_retained_when_image_options_are_disabled() {
    let raw = RawItem {
        link: "https://publisher.invalid/paper.pdf".into(),
        ..Default::default()
    };
    let hydrated = hydrate_new_mirror_companions_with(
        raw,
        false,
        false,
        false,
        false,
        true,
        |mut raw| async {
            raw.document = Some(crate::document::Asset {
                source_url: raw.link.clone(),
                bytes: b"%PDF-1.7\nfixture".to_vec(),
            });
            raw
        },
    )
    .await;
    assert!(hydrated.document.is_some());
}

#[tokio::test]
async fn known_subscription_wall_tries_archives_after_publisher_denies_a_fresh_cache() {
    crate::http::install_crypto_provider();
    let publisher = MockServer::start();
    let archives = MockServer::start();
    let denied = publisher.mock(|when, then| {
        when.method(GET).path("/article");
        then.status(403);
    });
    let available = archives.mock(|when, then| {
        when.method(GET)
            .path("/available")
            .query_param("url", publisher.url("/article"))
            .header_missing("authorization");
        then.status(200)
            .json_body(serde_json::json!({"archived_snapshots":{}}));
    });
    let lookup = archives.mock(|when, then| {
        when.method(GET)
            .path("/lookup")
            .header_missing("authorization");
        then.status(404);
    });
    let configured = source();
    let client = http::Client::new(&crate::config::FetchConfig {
        retries: 0,
        ..Default::default()
    })
    .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let failures = ArticleFailures::default();
    let mut raw = RawItem {
        title: "Article".into(),
        link: publisher.url("/article"),
        summary: Some("A meaningful publisher summary.".into()),
        ..Default::default()
    };
    let (_, kind) = heavy_content(&raw, &configured, &client, cache.path(), &failures).await;
    assert_eq!(kind, ContentKind::None);
    available.assert_calls(0);
    lookup.assert_calls(0);
    raw.extra
        .insert("subscription_required".into(), true.into());
    let (enriched, kind) = super::archive::with_test_endpoints(
        archives.url("/available").parse().unwrap(),
        archives.url("/lookup").parse().unwrap(),
        heavy_content(&raw, &configured, &client, cache.path(), &failures),
    )
    .await;
    assert_eq!(kind, ContentKind::None);
    assert_eq!(
        enriched.extra["subscription_required"].as_bool(),
        Some(true)
    );
    assert_eq!(enriched.summary, raw.summary);
    available.assert_calls(1);
    lookup.assert_calls(1);
    denied.assert_calls(1);
}

#[tokio::test]
async fn media_backfill_preserves_prose_and_never_refetches_feeds_or_articles() {
    crate::http::install_crypto_provider();
    let server = MockServer::start_async().await;
    let feed = server
        .mock_async(|when, then| {
            when.path("/feed");
            then.status(500);
        })
        .await;
    let article = server
        .mock_async(|when, then| {
            when.path("/article");
            then.status(500);
        })
        .await;
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(80, 40)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let image = server
        .mock_async(|when, then| {
            when.path("/picture.png");
            then.status(200)
                .header("content-type", "image/png")
                .body(png.into_inner());
        })
        .await;
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(root.path()));
    let front = FrontMatter {
        title: "Hand-edited title".into(),
        link: server.url("/article"),
        source: "blog".into(),
        labels: vec!["hand-edited".into()],
        hidden: true,
        first_seen: Utc::now(),
        ..Default::default()
    };
    let body = format!(
        "Hand-edited prose stays byte for byte.\n\n![Diagram]({})\n",
        server.url("/picture.png")
    );
    let html = format!(
        "<p>Original capture</p><img src='{}'>",
        server.url("/picture.png")
    );
    store
        .write_item(NewItem {
            dir: "items/blog",
            stem: "article",
            front: &front,
            body: &body,
            html: Some(&html),
            preview: None,
            images: &[],
        })
        .unwrap();
    let configured = Source {
        content: ContentMode::Light,
        previews: crate::config::PreviewPolicy::Remote,
        documents: crate::config::DocumentPolicy::Remote,
        engine: Engine::Feed {
            url: Url::parse(&server.url("/feed")).unwrap(),
        },
        ..source()
    };
    let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
    let failures = ArticleFailures::default();
    for changed in [true, false] {
        let (_, archive) = index_archive(store.items().unwrap());
        let opts = Options {
            backfill_media: true,
            existing_paths: Arc::new(OnceCell::new_with(Some(archive))),
            ..options()
        };
        let report = fetch_one(
            &configured,
            FetchOneContext {
                store: &store,
                store_root: root.path(),
                client: &client,
                cache_dir: cache.path(),
                options: &opts,
                article_failures: &failures,
                state_policy: StatePolicy::PersistentBranch,
            },
        )
        .await
        .unwrap();
        assert_eq!(!report.unchanged, changed);
    }
    let retained = store.read_item("items/blog/article").unwrap();
    assert_eq!(retained.body, body);
    assert_eq!(retained.front.title, front.title);
    assert_eq!(retained.front.labels, front.labels);
    assert_eq!(retained.front.first_seen, front.first_seen);
    assert!(retained.front.hidden);
    assert_eq!(
        fs::read_to_string(root.path().join("items/blog/article.html")).unwrap(),
        html
    );
    assert_eq!(retained.front.images.len(), 1);
    assert!(retained.front.images[0].variants.is_empty());
    assert_eq!(
        retained.front.remote_preview.as_ref().unwrap().url,
        server.url("/picture.png")
    );
    feed.assert_calls_async(0).await;
    article.assert_calls_async(0).await;
    image.assert_calls_async(1).await;
}

#[tokio::test]
async fn undated_policy_evictions_keep_capture_time_when_limits_change() {
    crate::http::install_crypto_provider();
    let server = MockServer::start_async().await;
    let feed = server.mock_async(|when, then| {
        when.path("/feed");
        then.status(200).header("content-type", "application/feed+json").json_body(serde_json::json!({
            "version": "https://jsonfeed.org/version/1.1", "title": "Undated feed",
            "items": [{"id":"old", "title":"Undated article", "url":server.url("/old"), "content_text":"Retained feed body."}]
        }));
    }).await;
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(root.path()));
    let mut configured = Source {
        content: ContentMode::Light,
        images: crate::config::ImagePolicy::Remote,
        limits: crate::config::Limits {
            max_items: 0,
            max_age_days: 0,
            max_bytes: 0,
            since: None,
        },
        engine: Engine::Feed {
            url: Url::parse(&server.url("/feed")).unwrap(),
        },
        ..source()
    };
    let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
    let first_seen = options().now;
    for stage in 0..3 {
        if stage == 1 {
            let item = store.items().unwrap().remove(0);
            configured.limits.max_items = 1;
            configured.limits.max_age_days = 1;
            store
                .record_evictions(
                    std::slice::from_ref(&item),
                    std::slice::from_ref(&item.path),
                    std::slice::from_ref(&configured),
                    &crate::config::Defaults::default(),
                )
                .unwrap();
            store.remove_item(&item.path).unwrap();
            configured.limits.max_items = 2;
        } else if stage == 2 {
            configured.limits.max_age_days = 0;
        }
        let (_, archive) = index_archive(store.items().unwrap());
        let options = Options {
            now: first_seen + chrono::Duration::days(if stage == 0 { 0 } else { 10 }),
            existing_paths: Arc::new(OnceCell::new_with(Some(archive))),
            ..options()
        };
        fetch_one(
            &configured,
            FetchOneContext {
                store: &store,
                store_root: root.path(),
                client: &client,
                cache_dir: cache.path(),
                options: &options,
                article_failures: &ArticleFailures::default(),
                state_policy: StatePolicy::PersistentBranch,
            },
        )
        .await
        .unwrap();
        let items = store.items().unwrap();
        if stage == 1 {
            assert!(
                items.is_empty(),
                "a count change cannot reset an undated article's age"
            );
        } else {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].front.first_seen, first_seen);
        }
    }
    feed.assert_calls_async(3).await;
    assert!(!root.path().join("sources/blog/evicted.json").exists());
}

#[tokio::test]
async fn source_limits_skip_article_and_media_requests_outside_the_selected_items() {
    crate::http::install_crypto_provider();
    let server = MockServer::start_async().await;
    let feed = server.mock_async(|when, then| {
        when.path("/feed");
        then.status(200).header("content-type", "application/feed+json").json_body(serde_json::json!({
            "version": "https://jsonfeed.org/version/1.1",
            "title": "Bounded feed",
            "items": (1..=3).map(|day| serde_json::json!({
                "id": format!("story-{day}"),
                "title": format!("Story {day}"),
                "url": server.url(format!("/story-{day}")),
                "date_published": format!("2026-09-0{day}T12:00:00Z"),
                "content_html": format!("<p>Article body.</p><img src='{}'>", server.url(format!("/picture-{day}.png"))),
            })).collect::<Vec<_>>()
        }));
    }).await;
    let skipped = server
        .mock_async(|when, then| {
            when.path_matches(regex::Regex::new("^/(story-[12]|picture-[12]\\.png)$").unwrap());
            then.status(500);
        })
        .await;
    let article = server.mock_async(|when, then| {
        when.path("/story-3");
        then.status(200).header("content-type", "text/html").body(format!(
            "<article><h1>Story 3</h1><p>{}</p><img src='{}'></article>",
            "A substantial article explains the details and preserves enough context to read independently. ".repeat(8),
            server.url("/picture-3.png")
        ));
    }).await;
    let mut image = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(640, 400)
        .write_to(&mut image, image::ImageFormat::Png)
        .unwrap();
    let image = server
        .mock_async(|when, then| {
            when.path("/picture-3.png");
            then.status(200)
                .header("content-type", "image/png")
                .body(image.into_inner());
        })
        .await;
    let client = http::Client::new(&crate::config::FetchConfig {
        retries: 0,
        ..Default::default()
    })
    .unwrap();
    for limits in [
        crate::config::Limits {
            max_items: 1,
            ..Default::default()
        },
        crate::config::Limits {
            max_items: 0,
            since: Some("2026-09-03".parse().unwrap()),
            ..Default::default()
        },
    ] {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(root.path()));
        let configured = Source {
            limits,
            engine: Engine::Feed {
                url: Url::parse(&server.url("/feed")).unwrap(),
            },
            ..source()
        };
        let options = options();
        fetch_one(
            &configured,
            FetchOneContext {
                store: &store,
                store_root: root.path(),
                client: &client,
                cache_dir: cache.path(),
                options: &options,
                article_failures: &ArticleFailures::default(),
                state_policy: StatePolicy::PersistentBranch,
            },
        )
        .await
        .unwrap();
        let items = store.items().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].front.title, "Story 3");
        assert_eq!(items[0].front.images.len(), 1);
    }
    feed.assert_calls_async(2).await;
    article.assert_calls_async(2).await;
    image.assert_calls_async(2).await;
    skipped.assert_calls_async(0).await;
}

#[tokio::test]
async fn light_remote_defaults_fetch_only_the_feed_and_keep_preview_urls() {
    crate::http::install_crypto_provider();
    let server = MockServer::start_async().await;
    let feed = server.mock_async(|when, then| {
        when.path("/feed");
        then.status(200).header("content-type", "application/rss+xml").body(format!(
            "<rss version='2.0'><channel><title>Test</title><link>{0}</link><description>Test</description><item><guid>episode</guid><title>Episode</title><link>{0}/episode</link><enclosure url='{0}/audio.mp3' type='audio/mpeg' length='1'/><description><![CDATA[<p>Show notes.</p><img src='{0}/cover.png' alt='Cover'>]]></description></item><item><guid>paper</guid><title>Paper</title><link>{0}/paper.pdf</link><description>Paper summary.</description></item></channel></rss>", server.base_url()));
    }).await;
    let unused = server
        .mock_async(|when, then| {
            when.path_matches(
                regex::Regex::new("^/(episode|audio.mp3|cover.png|paper.pdf)$").unwrap(),
            );
            then.status(500);
        })
        .await;
    let config = crate::config::Config::parse(&format!(
        "[[sources]]\nurl={:?}\nslug='blog'\n",
        server.url("/feed")
    ))
    .unwrap();
    let configured = config.sources().unwrap().remove(0);
    let root = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(root.path()));
    let client = http::Client::new(&config.fetch).unwrap();
    let failures = ArticleFailures::default();
    for _ in 0..2 {
        let (_, archive) = index_archive(store.items().unwrap());
        let opts = Options {
            existing_paths: Arc::new(OnceCell::new_with(Some(archive))),
            ..options()
        };
        fetch_one(
            &configured,
            FetchOneContext {
                store: &store,
                store_root: root.path(),
                client: &client,
                cache_dir: cache.path(),
                options: &opts,
                article_failures: &failures,
                state_policy: StatePolicy::PersistentBranch,
            },
        )
        .await
        .unwrap();
    }
    let items = store.items().unwrap();
    assert_eq!(items.len(), 2);
    let episode = items
        .iter()
        .find(|item| item.front.title == "Episode")
        .unwrap();
    assert_eq!(
        episode.front.remote_preview.as_ref().unwrap().url,
        server.url("/cover.png")
    );
    assert!(episode.front.preview.is_none());
    assert!(
        items
            .iter()
            .all(|item| item.front.images.is_empty() && item.front.document.is_none())
    );
    feed.assert_calls_async(2).await;
    unused.assert_calls_async(0).await;
}
