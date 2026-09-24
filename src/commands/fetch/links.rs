//! Cross-source URL reservations. Overlapping feeds list the same article; the source that
//! reserves its links first stores it, and the others see a duplicate and leave no trace.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

#[derive(Default)]
pub(super) struct SharedLinks {
    state: Mutex<LinkState>,
    changed: Notify,
}

#[derive(Default)]
pub(super) struct LinkState {
    pub(super) committed: HashSet<String>,
    pub(super) reserved: HashSet<String>,
}

impl SharedLinks {
    pub(super) fn new(committed: HashSet<String>) -> Self {
        Self {
            state: Mutex::new(LinkState {
                committed,
                ..Default::default()
            }),
            changed: Notify::new(),
        }
    }

    pub(super) fn state(&self) -> std::sync::MutexGuard<'_, LinkState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

#[derive(Default)]
pub(super) struct LinkTransaction {
    shared: Option<Arc<SharedLinks>>,
    reserved: HashSet<String>,
}

impl LinkTransaction {
    pub(super) async fn reserve(&mut self, shared: &Arc<SharedLinks>, wanted: HashSet<String>) {
        loop {
            let changed = shared.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = shared.state();
                if wanted.is_disjoint(&state.reserved) {
                    self.reserved = wanted.difference(&state.committed).cloned().collect();
                    state.reserved.extend(self.reserved.iter().cloned());
                    self.shared = Some(shared.clone());
                    return;
                }
            }
            changed.await;
        }
    }

    pub(super) fn duplicate(&self, link: &str) -> bool {
        !link.is_empty() && !self.reserved.contains(link)
    }

    pub(super) fn commit(self) {
        if let Some(shared) = &self.shared {
            shared
                .state()
                .committed
                .extend(self.reserved.iter().cloned());
        }
    }
}

impl Drop for LinkTransaction {
    fn drop(&mut self) {
        if let Some(shared) = &self.shared {
            {
                let mut state = shared.state();
                for link in &self.reserved {
                    state.reserved.remove(link);
                }
            }
            shared.changed.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{options, source};
    use super::super::{ArticleFailures, FetchOneContext, Options, StatePolicy, fetch_one};
    use super::*;
    use crate::config::{ContentMode, Engine, Source};
    use crate::http;
    use crate::model::normalize_link;
    use crate::store::Store;
    use anyhow::Result;
    use httpmock::prelude::*;
    use std::fs;
    use url::Url;

    fn archived_links(store: &Store) -> Result<HashSet<String>> {
        Ok(store
            .items()?
            .into_iter()
            .map(|item| normalize_link(&item.front.link))
            .filter(|link| !link.is_empty())
            .collect())
    }

    #[tokio::test]
    async fn link_reservations_wait_only_for_overlap_and_retry_after_rollback() {
        use futures_util::FutureExt as _;

        let shared = Arc::new(SharedLinks::default());
        let mut first = LinkTransaction::default();
        first.reserve(&shared, HashSet::from(["a".into()])).await;
        let mut disjoint = LinkTransaction::default();
        disjoint
            .reserve(&shared, HashSet::from(["b".into()]))
            .now_or_never()
            .expect("unrelated articles must remain concurrent");
        disjoint.commit();
        let waiting_shared = shared.clone();
        let waiting = tokio::spawn(async move {
            let mut next = LinkTransaction::default();
            next.reserve(&waiting_shared, HashSet::from(["a".into(), "b".into()]))
                .await;
            assert!(!next.duplicate("a"));
            assert!(next.duplicate("b"));
            next.commit();
        });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        drop(first);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            shared.state().committed,
            HashSet::from(["a".into(), "b".into()])
        );
        assert!(shared.state().reserved.is_empty());
    }

    #[tokio::test]
    async fn concurrent_sources_store_a_shared_article_once_and_duplicates_leave_no_trace() {
        crate::http::install_crypto_provider();
        let server = MockServer::start_async().await;
        for (path, id, link) in [
            (
                "/publisher",
                "publisher-id",
                "https://www.example.com/article/",
            ),
            (
                "/aggregator",
                "hn-id",
                "http://example.com/article?utm_source=hn#comments",
            ),
        ] {
            server
                .mock_async(|when, then| {
                    when.path(path);
                    then.status(200)
                        .header("content-type", "application/feed+json")
                        .json_body(serde_json::json!({
                            "version": "https://jsonfeed.org/version/1.1",
                            "title": id,
                            "items": [{"id": id, "title": id, "url": link, "content_text": "Body"}]
                        }));
                })
                .await;
        }
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(root.path()));
        let client = http::Client::new(&crate::config::FetchConfig::default()).unwrap();
        let configured = |slug: &str| Source {
            slug: slug.into(),
            content: ContentMode::Light,
            images: crate::config::ImagePolicy::Remote,
            engine: Engine::Feed {
                url: Url::parse(&server.url(format!("/{slug}"))).unwrap(),
            },
            ..source()
        };
        let publisher = configured("publisher");
        let aggregator = configured("aggregator");
        let test_options = options();
        let failures = ArticleFailures::default();
        let context = FetchOneContext {
            store: &store,
            store_root: root.path(),
            client: &client,
            cache_dir: cache.path(),
            options: &test_options,
            article_failures: &failures,
            state_policy: StatePolicy::PersistentBranch,
        };
        let dry_options = Options {
            dry_run: true,
            ..options()
        };
        let dry_context = FetchOneContext {
            options: &dry_options,
            ..context
        };
        let (first, second) = tokio::join!(
            fetch_one(&publisher, dry_context),
            fetch_one(&aggregator, dry_context)
        );
        assert_eq!(first.unwrap().added + second.unwrap().added, 1);
        assert!(store.items().unwrap().is_empty());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        let (first, second) = tokio::join!(
            fetch_one(&publisher, context),
            fetch_one(&aggregator, context)
        );
        assert_eq!(first.unwrap().added + second.unwrap().added, 1);
        let archived = store.items().unwrap();
        assert_eq!(archived.len(), 1);
        let skipped = if archived[0].front.source == publisher.slug {
            &aggregator
        } else {
            &publisher
        };
        assert!(!root.path().join("sources").join(&skipped.slug).exists());
        let next_options = Options {
            archived_links: Arc::new(SharedLinks::new(archived_links(&store).unwrap())),
            ..options()
        };
        let next_context = FetchOneContext {
            options: &next_options,
            ..context
        };
        assert_eq!(fetch_one(skipped, next_context).await.unwrap().added, 0);
        assert!(!root.path().join("sources").join(&skipped.slug).exists());
        let owner = if skipped.slug == publisher.slug {
            &aggregator
        } else {
            &publisher
        };
        let refresh_options = Options {
            refresh: true,
            ..next_options.clone()
        };
        assert_eq!(
            fetch_one(
                owner,
                FetchOneContext {
                    options: &refresh_options,
                    ..context
                }
            )
            .await
            .unwrap()
            .added,
            1
        );
        assert_eq!(store.items().unwrap().len(), 1);
    }
}
