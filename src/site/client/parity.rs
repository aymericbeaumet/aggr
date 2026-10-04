//! Parity fixtures: the markup the minijinja partials render for known models, written to
//! `web/test/parity/` so the Svelte components can prove they render the same thing. Run
//! `AGGR_UPDATE_PARITY=1 cargo test --bin aggr parity` after changing a partial or the model;
//! without the variable the test fails when the committed fixtures are stale.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone as _, Utc};
use scraper::{ElementRef, Html, Selector};

use super::{ClientPage, ClientRow, ClientSite, ClientView, Shared};
use crate::config::preferences::ReaderPreferences;
use crate::content;
use crate::model::{ContentKind, FrontMatter, Item};
use crate::site::context::{
    BuildCtx, CategoryCtx, DiscussionLinkCtx, ItemCtx, ItemOptions, PreviewCtx, SiteCtx, SourceCtx,
    SourceErrorCtx, SourceMembershipCtx, og_locale,
};
use crate::site::page::{ListPage, Pages, SharedCtx, SimplePage};
use crate::site::render::{Renderer, Theme};
use crate::site::{outputs, paginate};

struct Fixture {
    name: &'static str,
    component: &'static str,
    props: serde_json::Value,
    html: String,
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap()
}

fn site(excerpts: bool) -> SiteCtx {
    let mut params = toml::Table::new();
    params.insert("excerpts".into(), toml::Value::Boolean(excerpts));
    SiteCtx {
        title: "Reader".into(),
        description: String::new(),
        identity: None,
        language: "en".into(),
        og_locale: og_locale("en"),
        base_path: "/".into(),
        base_url: None,
        indexing: true,
        repository: None,
        data_branch: "aggr".into(),
        network_url: outputs::AGGR_NETWORK,
        instance_type_url: outputs::AGGR_INSTANCE_TYPE,
        pwa: false,
        preferences: serde_json::json!({}),
        preference_schema: ReaderPreferences::default().schema(),
        config_page_url: None,
        config_url: None,
        has_categories: true,
        discussions: vec![DiscussionLinkCtx {
            name: "hackernews".into(),
            url: "https://hn.algolia.com/?q={url}".into(),
            shortcut: Some("H".into()),
            found: false,
            score: None,
        }],
        entry_shortcuts: Vec::new(),
        params,
    }
}

fn build() -> BuildCtx {
    BuildCtx {
        time: now(),
        version: "1.0.0".into(),
        app_version: "app-fixture".into(),
        content_version: "content-fixture".into(),
        config_sha: None,
        data_sha: None,
        generation: "generation-fixture".into(),
        release: false,
    }
}

fn placeholder() -> crate::media::placeholder::Placeholder {
    crate::media::placeholder::from_image(&image::DynamicImage::new_rgb8(4, 4)).unwrap()
}

fn preview(alt: Option<&str>) -> PreviewCtx {
    PreviewCtx {
        url: "assets/previews/fixture.jpg".into(),
        width: 320,
        height: 180,
        alt: alt.map(str::to_string),
        color: Some("#285a8c".into()),
        placeholder: placeholder(),
    }
}

/// An item `hours_ago` old, captured with a readable body.
fn item(stem: &str, title: &str, hours_ago: i64, body: &str) -> Item {
    let date = now() - chrono::Duration::hours(hours_ago);
    Item {
        path: format!("items/blog/2026/09/{stem}"),
        front: FrontMatter {
            title: title.into(),
            link: format!("https://blog.example/{stem}"),
            source: "blog".into(),
            published: Some(date),
            first_seen: date,
            content: if body.is_empty() {
                ContentKind::None
            } else {
                ContentKind::Extracted
            },
            ..Default::default()
        },
        body: body.into(),
    }
}

fn context(item: &Item, category: Option<&str>, excerpt: &str) -> ItemCtx {
    ItemCtx::from_item(
        item,
        ItemOptions {
            source_name: "Blog",
            reading_metrics: content::reading_metrics(&item.body),
            category,
            links: None,
            excerpt: excerpt.into(),
            discussions: &[],
            resolutions: &crate::discussions::ResolutionSet::default(),
            now: now(),
        },
    )
}

fn refresh(ctx: &mut ItemCtx) {
    ctx.metadata = crate::site::display::Metadata::from(&*ctx);
}

const BODY: &str = "Paragraph one of the fixture article, long enough to count words for the reading \
estimate that rows and headers show.\n\nParagraph two keeps the estimate above a minute.";

/// The row variants a list can show: every optional metadata field on and off.
fn rows() -> Vec<(&'static str, ItemCtx)> {
    let mut plain = context(&item("plain", "A plain article", 2, BODY), None, "");
    refresh(&mut plain);

    let mut full = context(
        &item(
            "full",
            "Everything on: preview, category, via & discussion",
            5,
            BODY,
        ),
        Some("Engineering"),
        "An excerpt shown under the title when the site enables excerpts.",
    );
    full.language = Some("fr".into());
    full.labels = vec!["rust".into(), "web".into()];
    full.updated = Some(now() - chrono::Duration::hours(1));
    full.preview = Some(preview(Some("A small card")));
    full.is_aggregated = true;
    full.feed_display = "Planet Example".into();
    full.source_memberships = vec![
        SourceMembershipCtx {
            slug: "blog".into(),
            query_value: "blog.example".into(),
            name: "Blog".into(),
            display: "blog.example".into(),
        },
        SourceMembershipCtx {
            slug: "planet".into(),
            query_value: "planet.example".into(),
            name: "Planet Example".into(),
            display: "planet.example".into(),
        },
    ];
    full.discussions = vec![DiscussionLinkCtx {
        name: "hackernews".into(),
        url: "https://news.ycombinator.com/item?id=1".into(),
        shortcut: Some("H".into()),
        found: true,
        score: Some(120),
    }];
    full.extra.insert("points".into(), 120.into());
    full.extra.insert(
        "comments_url".into(),
        "https://news.ycombinator.com/item?id=1".into(),
    );
    full.extra.insert("num_comments".into(), 42.into());
    refresh(&mut full);

    let mut podcast = context(
        &item("podcast", "An episode", 30, BODY),
        None,
        "Show notes.",
    );
    podcast.item_type = crate::site::item_type::ItemType::Podcast;
    podcast.extra.insert("duration_seconds".into(), 3725.into());
    podcast.preview = Some(preview(None));
    refresh(&mut podcast);

    let mut video = context(&item("video", "A recording", 200, ""), None, "");
    video.item_type = crate::site::item_type::ItemType::Video;
    refresh(&mut video);

    let mut comments_only = context(
        &item("comments", "Comments without a count or points", 0, BODY),
        None,
        "",
    );
    comments_only
        .extra
        .insert("comments_url".into(), "https://lobste.rs/s/fixture".into());
    refresh(&mut comments_only);

    let mut escaped = context(
        &item(
            "escaped",
            "Title with </script> & <b>markup</b> \"quotes\"",
            8,
            BODY,
        ),
        None,
        "Excerpt with </script> & <i>markup</i>",
    );
    refresh(&mut escaped);

    vec![
        ("row-plain", plain),
        ("row-full", full),
        ("row-podcast", podcast),
        ("row-video", video),
        ("row-comments", comments_only),
        ("row-escaped", escaped),
    ]
}

fn source(error: bool) -> SourceCtx {
    SourceCtx {
        slug: "blog".into(),
        query_value: "blog.example".into(),
        name: "Blog".into(),
        url: Some("https://blog.example/feed.xml".into()),
        feed_url: Some("https://blog.example/feed.xml".into()),
        site_url: Some("https://blog.example/".into()),
        language: None,
        category: None,
        engine: "feed".into(),
        count: 1,
        latest: Some(now()),
        error: error.then(|| SourceErrorCtx {
            message: "HTTP 503".into(),
            since: now() - chrono::Duration::days(2),
        }),
        page: "sources/blog/".into(),
        listed: true,
    }
}

fn category() -> CategoryCtx {
    CategoryCtx {
        name: "Engineering".into(),
        slug: "engineering".into(),
        count: 1,
        latest: Some(now()),
        page: "categories/engineering/".into(),
    }
}

struct Rendered {
    document: Html,
    model: ClientPage,
    client_site: ClientSite,
}

#[allow(clippy::too_many_arguments)]
fn render_list(
    site: &SiteCtx,
    kind: &str,
    title: &str,
    prefix: &str,
    items: &[ItemCtx],
    source: Option<&SourceCtx>,
    category: Option<&CategoryCtx>,
    per_page: usize,
    pager_index: usize,
) -> Rendered {
    let build = build();
    let renderer = Renderer::new(Theme::default()).unwrap();
    let sources: Vec<_> = source.cloned().into_iter().collect();
    let categories: Vec<_> = category.cloned().into_iter().collect();
    let shared = SharedCtx::new(site, &build, &sources, &categories, &[]);
    let client = Shared::new(site, &build);
    let pages = Pages::new(site, &renderer, shared, items, per_page);
    let list = ListPage {
        kind,
        title,
        prefix,
        list: items,
        source,
        category,
    };
    let pagers = paginate(prefix, items.len(), per_page);
    let pager = &pagers[pager_index];
    let html = pages.list(&list, pager).unwrap();
    let page = pages.list_page(&list, pager);
    let model = ClientPage::list(
        &client,
        &page,
        &pager.context,
        &items[pager.range.clone()],
        source,
        category,
    );
    Rendered {
        document: Html::parse_document(&html),
        model,
        client_site: client.site,
    }
}

fn render_article(site: &SiteCtx, item: &ItemCtx) -> Rendered {
    let build = build();
    let renderer = Renderer::new(Theme::default()).unwrap();
    let shared = SharedCtx::new(site, &build, &[], &[], &[]);
    let client = Shared::new(site, &build);
    let archive = vec![item.clone()];
    let pages = Pages::new(site, &renderer, shared, &archive, 50);
    let mut simple = SimplePage::new("item", &item.title, &item.url, "item.html");
    simple.item = Some(item);
    let page = pages.simple_page(&simple);
    let html = pages.simple(simple).unwrap();
    let model = ClientPage::simple(&client, &page, Some(item), site);
    Rendered {
        document: Html::parse_document(&html),
        model,
        client_site: client.site,
    }
}

fn select<'a>(document: &'a Html, selector: &str) -> Vec<ElementRef<'a>> {
    document
        .select(&Selector::parse(selector).unwrap())
        .collect()
}

/// The markup inside the client's root: what `App` renders (boot creates the root itself).
fn inner(document: &Html, selector: &str) -> String {
    let matches = select(document, selector);
    assert_eq!(matches.len(), 1, "exactly one element matches {selector}");
    matches[0].inner_html()
}

fn outer(document: &Html, selector: &str) -> String {
    let matches = select(document, selector);
    assert!(!matches.is_empty(), "no element matches {selector}");
    matches
        .iter()
        .map(|element| element.html())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The article's chrome above its content: everything before the media figure or the body.
fn article_head(document: &Html) -> String {
    let article = select(document, "article.item")[0];
    let content = [
        "document-reader",
        "video-player",
        "native-video",
        "native-audio",
        "interactive-reader",
        "article-lead",
        "body",
    ];
    let mut parts = Vec::new();
    for child in article.children() {
        let Some(element) = ElementRef::wrap(child) else {
            continue;
        };
        let classes: Vec<_> = element.value().classes().collect();
        if element.value().attr("data-article-content").is_some()
            || classes.iter().any(|class| content.contains(class))
        {
            break;
        }
        parts.push(element.html());
    }
    parts.join("\n")
}

/// `html` with the children of the first element whose start tag begins with `open` removed,
/// keeping the element itself. Only `tag` is tracked for nesting, which is enough for the two
/// regions the client adopts rather than renders: they are `div`s and `main`s holding markup that
/// the server renderer never sees.
fn strip_children(html: &str, open: &str, tag: &str) -> String {
    let Some(start) = html.find(open) else {
        panic!("no element starts with {open}");
    };
    let content_start = start + html[start..].find('>').expect("start tag closes") + 1;
    let open_token = format!("<{tag}");
    let close_token = format!("</{tag}>");
    let mut depth = 1usize;
    let mut cursor = content_start;
    let content_end = loop {
        let rest = &html[cursor..];
        let next_open = rest.find(&open_token);
        let next_close = rest.find(&close_token).expect("element closes");
        match next_open {
            Some(open_at) if open_at < next_close => {
                depth += 1;
                cursor += open_at + open_token.len();
            }
            _ => {
                depth -= 1;
                if depth == 0 {
                    break cursor + next_close;
                }
                cursor += next_close + close_token.len();
            }
        }
    };
    format!("{}{}", &html[..content_start], &html[content_end..])
}

fn render_static(site: &SiteCtx, kind: &str, title: &str, path: &str, template: &str) -> Rendered {
    let build = build();
    let renderer = Renderer::new(Theme::default()).unwrap();
    let shared = SharedCtx::new(site, &build, &[], &[], &[]);
    let client = Shared::new(site, &build);
    let pages = Pages::new(site, &renderer, shared, &[], 50);
    let simple = SimplePage::new(kind, title, path, template);
    let page = pages.simple_page(&simple);
    let html = pages.simple(simple).unwrap();
    let model = ClientPage::simple(&client, &page, None, site);
    Rendered {
        document: Html::parse_document(&html),
        model,
        client_site: client.site,
    }
}

fn fixtures() -> Vec<Fixture> {
    let site_on = site(true);
    let site_off = site(false);
    let mut out = Vec::new();

    for (name, row) in rows() {
        for (suffix, site) in [("", &site_on), ("-no-excerpts", &site_off)] {
            if !suffix.is_empty() && row.excerpt.is_empty() {
                continue;
            }
            let rendered = render_list(
                site,
                "river",
                "Reader",
                "",
                std::slice::from_ref(&row),
                None,
                None,
                50,
                0,
            );
            let ClientView::List(list) = &rendered.model.page else {
                unreachable!()
            };
            out.push(Fixture {
                name: Box::leak(format!("{name}{suffix}").into_boxed_str()),
                component: "Row",
                props: serde_json::json!({
                    "row": list.rows[0],
                    "rank": 1,
                    "base": rendered.model.base,
                    "excerpts": rendered.client_site.excerpts,
                    "language": rendered.client_site.language,
                }),
                html: outer(&rendered.document, "ol.rows > li.row"),
            });
        }
    }

    // A long river: the pager on the first, a middle and the last page.
    let many: Vec<_> = (0..7)
        .map(|index| {
            let mut ctx = context(
                &item(
                    &format!("many-{index}"),
                    &format!("Entry {index}"),
                    index * 3,
                    BODY,
                ),
                None,
                "",
            );
            refresh(&mut ctx);
            ctx
        })
        .collect();
    for (name, index) in [("pager-first", 0), ("pager-middle", 1), ("pager-last", 3)] {
        let rendered = render_list(&site_on, "river", "Reader", "", &many, None, None, 2, index);
        let ClientView::List(list) = &rendered.model.page else {
            unreachable!()
        };
        out.push(Fixture {
            name,
            component: "Pager",
            props: serde_json::json!({
                "paginator": list.paginator,
                "kind": rendered.model.kind,
                "base": rendered.model.base,
            }),
            html: outer(&rendered.document, "[data-static-feed] > nav.pager"),
        });
    }
    let rendered = render_list(
        &site_on,
        "river",
        "Reader",
        "",
        &many[..1],
        None,
        None,
        50,
        0,
    );
    let ClientView::List(list) = &rendered.model.page else {
        unreachable!()
    };
    out.push(Fixture {
        name: "pager-single",
        component: "Pager",
        props: serde_json::json!({
            "paginator": list.paginator,
            "kind": rendered.model.kind,
            "base": rendered.model.base,
        }),
        html: outer(&rendered.document, "[data-static-feed] > nav.pager"),
    });

    // Scoped lists: their heading, error notice, toolbar and pager.
    let scoped = [
        ("source", "Blog", "sources/blog/", Some(source(false)), None),
        (
            "source-error",
            "Blog",
            "sources/blog/",
            Some(source(true)),
            None,
        ),
        (
            "category",
            "Engineering",
            "categories/engineering/",
            None,
            Some(category()),
        ),
        (
            "tag",
            "Engineering",
            "tags/engineering/",
            None,
            Some(category()),
        ),
    ];
    for (name, title, prefix, source, category) in scoped {
        let kind = name.split('-').next().unwrap();
        let rendered = render_list(
            &site_on,
            kind,
            title,
            prefix,
            &many,
            source.as_ref(),
            category.as_ref(),
            3,
            1,
        );
        let ClientView::List(list) = &rendered.model.page else {
            unreachable!()
        };
        out.push(Fixture {
            name: Box::leak(format!("listhead-{name}").into_boxed_str()),
            component: "ListHead",
            props: serde_json::json!({
                "kind": rendered.model.kind,
                "scope": list.scope,
                "error": list.error,
            }),
            html: outer(&rendered.document, ".listhead"),
        });
        out.push(Fixture {
            name: Box::leak(format!("toolbar-{name}").into_boxed_str()),
            component: "Toolbar",
            props: serde_json::json!({
                "kind": rendered.model.kind,
                "scope": list.scope,
                "base": rendered.model.base,
            }),
            html: outer(&rendered.document, ".feed-toolbar, [data-search-results]"),
        });
        out.push(Fixture {
            name: Box::leak(format!("pager-{name}").into_boxed_str()),
            component: "Pager",
            props: serde_json::json!({
                "paginator": list.paginator,
                "kind": rendered.model.kind,
                "base": rendered.model.base,
            }),
            html: outer(&rendered.document, "[data-static-feed] > nav.pager"),
        });
        out.push(Fixture {
            name: Box::leak(format!("header-{name}").into_boxed_str()),
            component: "Header",
            props: serde_json::json!({
                "kind": rendered.model.kind,
                "base": rendered.model.base,
                "site": rendered.client_site,
            }),
            html: outer(&rendered.document, "header.top, nav.mobile-tabs"),
        });
    }
    let rendered = render_list(&site_on, "river", "Reader", "", &many, None, None, 50, 0);
    out.push(Fixture {
        name: "toolbar-river",
        component: "Toolbar",
        props: serde_json::json!({
            "kind": rendered.model.kind,
            "scope": null,
            "base": rendered.model.base,
        }),
        html: outer(&rendered.document, ".feed-toolbar, [data-search-results]"),
    });
    out.push(Fixture {
        name: "header-river",
        component: "Header",
        props: serde_json::json!({
            "kind": rendered.model.kind,
            "base": rendered.model.base,
            "site": rendered.client_site,
        }),
        html: outer(&rendered.document, "header.top, nav.mobile-tabs"),
    });

    // Articles: the header chrome and the footer cards.
    let (_, full) = rows().into_iter().nth(1).unwrap();
    let (_, plain) = rows().into_iter().next().unwrap();
    let mut article = full.clone();
    article.body_html = Some("<p>Body</p>".into());
    article.authors = vec!["Ada".into(), "Grace".into()];
    article.resources = vec![
        content::ResourceLink {
            label: "Code".into(),
            url: "https://github.com/example/repo".into(),
        },
        content::ResourceLink {
            label: "Paper".into(),
            url: "https://arxiv.org/abs/0000.00000".into(),
        },
    ];
    article.has_margin_notes = true;
    article.next_article = Some(ClientRow::from(&plain));
    article.recommended_articles = vec![ClientRow::from(&many[0]), ClientRow::from(&many[1])];
    refresh(&mut article);
    let mut titles_only = context(&item("titles", "Titles only", 3, ""), None, "");
    refresh(&mut titles_only);
    let mut missing = context(
        &item("missing", "Article text not captured", 1, ""),
        None,
        "",
    );
    missing.content = ContentKind::Feed;
    refresh(&mut missing);
    let mut subscription = context(&item("wall", "Behind a paywall", 4, BODY), None, "");
    subscription.body_html = Some("<p>Teaser</p>".into());
    subscription
        .extra
        .insert("subscription_required".into(), true.into());
    subscription.extra.insert(
        "archive_lookup_url".into(),
        "https://archive.example/search?q=wall".into(),
    );
    refresh(&mut subscription);
    let mut bare = plain.clone();
    bare.body_html = Some("<p>Body</p>".into());
    refresh(&mut bare);
    for (name, item, site) in [
        ("article-full", &article, &site_on),
        ("article-titles-only", &titles_only, &site_on),
        ("article-missing", &missing, &site_on),
        ("article-subscription", &subscription, &site_on),
        ("article-bare", &bare, &site_off),
    ] {
        let rendered = render_article(site, item);
        let ClientView::Article(model) = &rendered.model.page else {
            unreachable!()
        };
        out.push(Fixture {
            name,
            component: "ArticleHeader",
            props: serde_json::json!({
                "article": model,
                "base": rendered.model.base,
                "language": rendered.client_site.language,
            }),
            html: article_head(&rendered.document),
        });
        if item.next_article.is_some() || !item.recommended_articles.is_empty() {
            out.push(Fixture {
                name: Box::leak(format!("{name}-footer").into_boxed_str()),
                component: "ArticleFooter",
                props: serde_json::json!({
                    "next": model.next,
                    "recommended": model.recommended,
                    "base": rendered.model.base,
                    "excerpts": rendered.client_site.excerpts,
                }),
                html: outer(&rendered.document, "footer.article-footer"),
            });
        }
    }

    // Whole pages: what `App` renders inside `#app` (boot creates that root) for each
    // client-rendered or adopted kind.
    // Adopted regions (the article's content, a static page's main) are emptied on the fixture
    // side because the server renderer has no DOM to adopt them from.
    let rendered = render_list(&site_on, "river", "Reader", "", &many, None, None, 3, 0);
    out.push(Fixture {
        name: "page-river",
        component: "App",
        props: serde_json::json!({ "page": rendered.model }),
        html: inner(&rendered.document, "#app"),
    });
    let rendered = render_list(
        &site_on,
        "source",
        "Blog",
        "sources/blog/",
        &many,
        Some(&source(true)),
        None,
        3,
        1,
    );
    out.push(Fixture {
        name: "page-source-error",
        component: "App",
        props: serde_json::json!({ "page": rendered.model }),
        html: inner(&rendered.document, "#app"),
    });
    let rendered = render_article(&site_on, &article);
    out.push(Fixture {
        name: "page-article",
        component: "App",
        props: serde_json::json!({ "page": rendered.model }),
        html: strip_children(
            &inner(&rendered.document, "#app"),
            "<div data-article-content",
            "div",
        ),
    });
    let rendered = render_static(&site_on, "browse", "Browse", "browse/", "browse.html");
    out.push(Fixture {
        name: "page-browse",
        component: "App",
        props: serde_json::json!({ "page": rendered.model }),
        html: strip_children(&inner(&rendered.document, "#app"), "<main", "main"),
    });
    out
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("web/test/parity")
}

#[test]
fn parity_fixtures_match_the_templates() {
    let dir = fixture_dir();
    let update = std::env::var_os("AGGR_UPDATE_PARITY").is_some();
    let fixtures = fixtures();
    let mut names: BTreeMap<String, ()> = BTreeMap::new();
    let mut stale = Vec::new();
    for fixture in &fixtures {
        assert!(
            names.insert(fixture.name.to_string(), ()).is_none(),
            "duplicate fixture {}",
            fixture.name
        );
        let json = serde_json::to_string_pretty(&serde_json::json!({
            "component": fixture.component,
            "props": fixture.props,
        }))
        .unwrap()
            + "\n";
        for (extension, expected) in [("json", &json), ("html", &fixture.html)] {
            let path = dir.join(format!("{}.{extension}", fixture.name));
            if update {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, expected).unwrap();
            } else if std::fs::read_to_string(&path).ok().as_ref() != Some(expected) {
                stale.push(path.display().to_string());
            }
        }
    }
    if update {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_string();
            let generated = matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("json" | "html")
            );
            // Only this test's own outputs are pruned; the frontend keeps its test code here.
            if generated && !names.contains_key(&stem) {
                std::fs::remove_file(&path).unwrap();
            }
        }
    }
    assert!(
        stale.is_empty(),
        "stale parity fixtures (run `AGGR_UPDATE_PARITY=1 cargo test --bin aggr parity`):\n{}",
        stale.join("\n")
    );
}
