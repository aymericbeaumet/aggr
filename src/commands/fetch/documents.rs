//! Preserve publisher PDFs with bounded downloads and the same retry policy as page captures.
use anyhow::Result;
use url::Url;

use super::{
    FetchOneContext, SourceTransaction,
    plan::{Planned, persist_item},
    repair::CaptureRetries,
};
use crate::{
    config::{ContentMode, Engine, Source},
    document::Asset,
    http,
    model::RawItem,
};

pub(super) use crate::document::url;

pub(super) async fn capture(
    url: &Url,
    source: &Source,
    context: &FetchOneContext<'_>,
) -> Option<Asset> {
    if context.options.dry_run
        || source.content == ContentMode::Light
        || matches!(source.engine, Engine::Aggr { .. })
    {
        return None;
    }
    let retries = CaptureRetries::new(context.cache_dir);
    let key = format!("pdf:{url}");
    if !context.options.refresh && !retries.due(&key) {
        return None;
    }
    let response = context
        .client
        .get(http::Request {
            url,
            headers: http::source_headers(source, url),
            etag: None,
            last_modified: None,
        })
        .await;
    if let Ok(http::Response::Ok(body)) = response {
        let asset = Asset {
            source_url: url.to_string(),
            bytes: body.bytes,
        };
        let mime = body
            .content_type
            .as_deref()
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        if (mime.is_empty()
            || mime.eq_ignore_ascii_case("application/pdf")
            || mime.eq_ignore_ascii_case("application/octet-stream"))
            && crate::document::validate_stored(
                &asset.bytes,
                &asset.metadata("document"),
                "document",
            )
            .is_ok()
        {
            retries.succeeded(&key);
            return Some(asset);
        }
    }
    retries.failed(&key);
    log::debug!("document preservation unavailable: {url}");
    None
}

pub(super) async fn repair(
    source: &Source,
    context: &FetchOneContext<'_>,
    mut transaction: Option<&mut SourceTransaction>,
) -> Result<usize> {
    if context.options.dry_run
        || source.content == ContentMode::Light
        || matches!(source.engine, Engine::Aggr { .. })
    {
        return Ok(0);
    }
    let Some(paths) = context
        .options
        .existing_paths
        .get()
        .and_then(|index| index.documents.get(&source.slug))
    else {
        return Ok(0);
    };
    let retries = CaptureRetries::new(context.cache_dir);
    let mut attempted = 0;
    let mut repaired = 0;
    for path in paths {
        let item = context.store.read_item(path)?;
        if item.front.source != source.slug
            || item.front.replicated_at.is_some()
            || context
                .store
                .read_document(&item)?
                .is_some_and(|asset| crate::document::matches_item(&asset, &item))
        {
            continue;
        }
        let Some(url) = url(&item.front.link, &item.front.extra) else {
            continue;
        };
        if !context.options.refresh && !retries.due(&format!("pdf:{url}")) {
            continue;
        }
        if attempted == 8 {
            break;
        }
        attempted += 1;
        let Some(document) = capture(&url, source, context).await else {
            continue;
        };
        let mut planned = Planned::from_existing(item, path)?;
        planned.front.document = Some(document.metadata(&planned.stem));
        let raw = RawItem {
            document: Some(document),
            ..Default::default()
        };
        if let Some(transaction) = transaction.as_deref_mut() {
            transaction.track_item(&planned, &raw)?;
        }
        persist_item(context.store, context.options, planned, raw).await?;
        repaired += 1;
    }
    Ok(repaired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        commands::fetch::{
            ArticleFailures, Options, StatePolicy,
            index::index_archive,
            tests::{options, source},
        },
        model::FrontMatter,
        store::{NewItem, Store},
    };
    use httpmock::prelude::*;
    use std::sync::Arc;
    use tokio::sync::OnceCell;

    #[tokio::test]
    async fn document_repair_preserves_content_and_is_a_noop_after_success_or_failure() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        let mut response = server
            .mock_async(|when, then| {
                when.path("/paper.pdf");
                then.status(403);
            })
            .await;
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(root.path()));
        let front = FrontMatter {
            title: "Paper".into(),
            link: server.url("/paper.pdf"),
            source: "blog".into(),
            ..Default::default()
        };
        store
            .write_item(NewItem {
                dir: "items/blog",
                stem: "paper",
                front: &front,
                body: "Preserved abstract.\n",
                html: Some("<p>Preserved abstract.</p>"),
                preview: None,
                images: &[],
            })
            .unwrap();
        let original = std::fs::read(root.path().join("items/blog/paper.md")).unwrap();
        let configured = source();
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let (_, archive) = index_archive(store.items().unwrap());
        let mut opts = Options {
            existing_paths: Arc::new(OnceCell::new_with(Some(archive))),
            ..options()
        };
        let failures = ArticleFailures::default();
        let light = Source {
            content: ContentMode::Light,
            ..configured.clone()
        };
        opts.refresh = true;
        {
            let context = FetchOneContext {
                store: &store,
                store_root: root.path(),
                client: &client,
                cache_dir: cache.path(),
                options: &opts,
                article_failures: &failures,
                state_policy: StatePolicy::DevCache,
            };
            assert!(
                capture(&Url::parse(&front.link).unwrap(), &light, &context)
                    .await
                    .is_none()
            );
            assert_eq!(repair(&light, &context, None).await.unwrap(), 0);
            response.assert_calls_async(0).await;
        }
        opts.refresh = false;
        for _ in 0..2 {
            let context = FetchOneContext {
                store: &store,
                store_root: root.path(),
                client: &client,
                cache_dir: cache.path(),
                options: &opts,
                article_failures: &failures,
                state_policy: StatePolicy::DevCache,
            };
            assert_eq!(repair(&configured, &context, None).await.unwrap(), 0);
        }
        response.assert_calls_async(1).await;
        assert_eq!(
            std::fs::read(root.path().join("items/blog/paper.md")).unwrap(),
            original
        );
        response.delete_async().await;
        response = server
            .mock_async(|when, then| {
                when.path("/paper.pdf");
                then.status(200)
                    .header("content-type", "application/pdf")
                    .body("%PDF-1.7\nfixture");
            })
            .await;
        CaptureRetries::new(cache.path()).failed(&server.url("/paper.pdf"));
        CaptureRetries::new(cache.path()).succeeded(&format!("pdf:{}", server.url("/paper.pdf")));
        opts.refresh = false;
        let context = FetchOneContext {
            store: &store,
            store_root: root.path(),
            client: &client,
            cache_dir: cache.path(),
            options: &opts,
            article_failures: &failures,
            state_policy: StatePolicy::DevCache,
        };
        assert_eq!(repair(&configured, &context, None).await.unwrap(), 1);
        let repaired = std::fs::read(root.path().join("items/blog/paper.md")).unwrap();
        assert_eq!(repair(&configured, &context, None).await.unwrap(), 0);
        response.assert_calls_async(1).await;
        assert_eq!(
            std::fs::read(root.path().join("items/blog/paper.md")).unwrap(),
            repaired
        );
        let item = store.read_item("items/blog/paper").unwrap();
        assert_eq!(item.body, "Preserved abstract.\n");
        assert_eq!(
            std::fs::read_to_string(root.path().join("items/blog/paper.html")).unwrap(),
            "<p>Preserved abstract.</p>"
        );
        assert_eq!(
            store.read_document(&item).unwrap().unwrap().bytes,
            b"%PDF-1.7\nfixture"
        );
        assert!(
            !CaptureRetries::new(cache.path()).due(&server.url("/paper.pdf")),
            "document success does not reset HTML backoff"
        );
        let old_document = root
            .path()
            .join("items/blog")
            .join(&item.front.document.as_ref().unwrap().file);
        let mut changed = item.front.clone();
        changed.link = server.url("/replacement.pdf?version=2");
        store
            .write_item(NewItem {
                dir: "items/blog",
                stem: "paper",
                front: &changed,
                body: &item.body,
                html: None,
                preview: None,
                images: &[],
            })
            .unwrap();
        let replacement = server
            .mock_async(|when, then| {
                when.path("/replacement.pdf").query_param("version", "2");
                then.status(200)
                    .header("content-type", "application/pdf")
                    .body("%PDF-1.7\nreplacement");
            })
            .await;
        assert_eq!(repair(&configured, &context, None).await.unwrap(), 1);
        assert_eq!(repair(&configured, &context, None).await.unwrap(), 0);
        replacement.assert_calls_async(1).await;
        assert!(
            old_document.is_file(),
            "replacement does not remove old immutable bytes"
        );
        assert_eq!(
            store
                .read_document(&store.read_item("items/blog/paper").unwrap())
                .unwrap()
                .unwrap()
                .source_url,
            changed.link
        );
    }
}
