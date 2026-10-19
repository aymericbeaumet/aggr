//! Static Rust page fixtures for client DOM behavior tests. These preserve the markup and typed
//! model together; no browser component reimplements their templates. Refresh with
//! `AGGR_UPDATE_PARITY=1 cargo test --bin aggr parity` after changing a template or model.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone as _, Utc};
use scraper::{ElementRef, Html, Selector};

use super::{ClientPage, ClientRow, Shared};
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
    props: serde_json::Value,
    html: String,
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap()
}

fn site() -> SiteCtx {
    SiteCtx {
        hermetic: false,
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
        "A stored excerpt used by descriptions and feeds, not by the row.",
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

    vec![("row-plain", plain), ("row-full", full)]
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

struct Rendered {
    document: Html,
    model: ClientPage,
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
    }
}

fn select<'a>(document: &'a Html, selector: &str) -> Vec<ElementRef<'a>> {
    document
        .select(&Selector::parse(selector).unwrap())
        .collect()
}

/// The static markup inside the root the client enhances.
fn inner(document: &Html, selector: &str) -> String {
    let matches = select(document, selector);
    assert_eq!(matches.len(), 1, "exactly one element matches {selector}");
    matches[0].inner_html()
}

/// `html` with the children of the first element whose start tag begins with `open` removed,
/// keeping the element itself. Only `tag` is tracked for nesting, which is enough for the two
/// fixture placeholders, both `div` and `main` elements.
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
    }
}

fn fixtures() -> Vec<Fixture> {
    let site = site();
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
    let mut rows = rows().into_iter();
    let (_, plain) = rows.next().unwrap();
    let (_, mut article) = rows.next().unwrap();
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
    let mut out = Vec::new();
    let rendered = render_list(&site, "river", "Reader", "", &many, None, None, 3, 0);
    out.push(Fixture {
        name: "page-river",
        props: serde_json::json!({ "page": rendered.model }),
        html: inner(&rendered.document, "#app"),
    });
    let rendered = render_list(
        &site,
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
        props: serde_json::json!({ "page": rendered.model }),
        html: inner(&rendered.document, "#app"),
    });
    let rendered = render_article(&site, &article);
    out.push(Fixture {
        name: "page-article",
        props: serde_json::json!({ "page": rendered.model }),
        html: strip_children(
            &inner(&rendered.document, "#app"),
            "<div data-article-content",
            "div",
        ),
    });
    let rendered = render_static(&site, "browse", "Browse", "browse/", "browse.html");
    out.push(Fixture {
        name: "page-browse",
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
