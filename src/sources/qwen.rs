//! The public article endpoint behind Qwen's JavaScript-rendered research blog.

use super::{Context, Fetch, SourceMeta, Validators};
use crate::model::RawItem;
use crate::{
    config::Source,
    http::{Request, Response},
    model::sha1_hex,
};
use anyhow::{Context as _, Result, ensure};
use serde_json::Value;
use url::Url;

pub fn is_blog_url(url: &Url) -> bool {
    url.host_str() == Some("qwen.ai")
        && matches!(url.path().trim_end_matches('/'), "/blog" | "/research")
        && url.query().is_none()
}

pub async fn fetch(url: &Url, source: &Source, ctx: &Context<'_>) -> Result<Fetch> {
    let endpoint = url.join("/api/v2/article/retrieval?type=qwen_ai&language=en-US")?;
    let previous = if ctx.state.identity == source.identity {
        Validators::from_state(ctx.state)
    } else {
        Validators::default()
    };
    let response = ctx
        .client
        .get(Request {
            url: &endpoint,
            headers: crate::http::source_headers(source, &endpoint),
            etag: previous.etag.as_deref(),
            last_modified: previous.last_modified.as_deref(),
        })
        .await?;
    let Response::Ok(body) = response else {
        return Ok(Fetch::Unchanged {
            validators: previous,
        });
    };
    let previous_hash = previous.body_hash.clone();
    let (body, parsed) = tokio::task::spawn_blocking(move || {
        let parsed = parse(&body.bytes, previous_hash.as_deref())?;
        Ok::<_, anyhow::Error>((body, parsed))
    })
    .await
    .context("parsing Qwen articles worker")??;
    let (body_hash, items) = parsed;
    let validators = Validators {
        etag: body.etag,
        last_modified: body.last_modified,
        body_hash: Some(body_hash),
        resolved_url: Some(endpoint.to_string()),
    };
    let Some(items) = items else {
        return Ok(Fetch::Unchanged { validators });
    };
    Ok(Fetch::Changed {
        validators,
        meta: SourceMeta {
            title: Some("Qwen".into()),
            site_url: Some("https://qwen.ai/".into()),
        },
        items,
    })
}

fn parse(bytes: &[u8], previous_hash: Option<&str>) -> Result<(String, Option<Vec<RawItem>>)> {
    let document: Value = serde_json::from_slice(bytes).context("parsing Qwen article API")?;
    ensure!(
        document["success"] == true,
        "Qwen article API reported failure"
    );
    let articles = document["data"]["articles"]
        .as_array()
        .context("Qwen article API has no articles list")?;
    // The API generates a new request ID for every response, even with unchanged articles.
    let hash = sha1_hex(&serde_json::to_vec(articles)?);
    if Some(hash.as_str()) == previous_hash {
        return Ok((hash, None));
    }
    let items = articles.iter().map(convert).collect::<Result<Vec<_>>>()?;
    Ok((hash, Some(items)))
}

fn convert(article: &Value) -> Result<RawItem> {
    let path = article["path"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .context("Qwen article has no path")?;
    let title = article["title"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .context("Qwen article has no title")?;
    let mut link = Url::parse("https://qwen.ai/blog")?;
    link.query_pairs_mut().append_pair("id", path);
    let extra = &article["extra"];
    let html = article["content"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .context("Qwen article has no content")?;
    let document = scraper::Html::parse_document(html);
    let content = [".post-content", "article", "main", "body"]
        .into_iter()
        .find_map(|selector| {
            let selector = scraper::Selector::parse(selector).ok()?;
            document
                .select(&selector)
                .next()
                .map(|element| element.inner_html())
        })
        .unwrap_or_else(|| html.to_string());
    Ok(RawItem {
        id: Some(path.to_string()),
        title: title.to_string(),
        link: link.to_string(),
        published: extra["date"]
            .as_str()
            .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
            .map(|date| date.to_utc()),
        authors: extra["author"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .into_iter()
            .collect(),
        labels: extra["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        content_html: Some(content),
        preview_candidates: extra["cover_small"]
            .as_str()
            .and_then(|value| Url::parse(value).ok())
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .map(|url| crate::preview::Candidate {
                url: url.to_string(),
                alt: None,
            })
            .into_iter()
            .collect(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(request_id: &str) -> Value {
        json!({"success":true,"request_id":request_id,"data":{"articles":[{
            "id":"uuid","path":"qwen-release","title":"Qwen release","language":"en-US",
            "content":"<html><body><nav>Navigation</nav><article><div class='post-content'><p>Actual research</p><img src='https://images.example/chart.png'></div></article><footer>Footer</footer></body></html>",
            "extra":{"date":"2026-08-03T10:00:00+08:00","author":"Qwen Team","tags":["Research"],"cover_small":"https://images.example/cover.png"}
        }]}})
    }

    #[test]
    fn converts_current_api_posts_with_content_and_metadata() {
        let (_, items) = parse(&serde_json::to_vec(&payload("a")).unwrap(), None).unwrap();
        let items = items.unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.link, "https://qwen.ai/blog?id=qwen-release");
        assert_eq!(item.title, "Qwen release");
        assert_eq!(
            item.published.unwrap().to_rfc3339(),
            "2026-08-03T02:00:00+00:00"
        );
        assert_eq!(item.authors, ["Qwen Team"]);
        assert_eq!(item.labels, ["Research"]);
        let html = item.content_html.as_ref().unwrap();
        assert!(html.contains("Actual research"));
        assert!(html.contains("chart.png"));
        assert!(!html.contains("Navigation"));
        assert!(!html.contains("Footer"));
    }

    #[test]
    fn changing_request_ids_does_not_invalidate_article_cache() {
        let first = parse(&serde_json::to_vec(&payload("a")).unwrap(), None)
            .unwrap()
            .0;
        let second = parse(&serde_json::to_vec(&payload("b")).unwrap(), None)
            .unwrap()
            .0;
        assert_eq!(first, second);
        assert!(
            parse(&serde_json::to_vec(&payload("b")).unwrap(), Some(&first))
                .unwrap()
                .1
                .is_none()
        );
        let mut changed = payload("b");
        changed["data"]["articles"][0]["title"] = json!("New title");
        assert_ne!(
            first,
            parse(&serde_json::to_vec(&changed).unwrap(), None)
                .unwrap()
                .0
        );
    }

    #[test]
    fn rejects_api_failures_and_preserves_empty_lists() {
        assert!(parse(br#"{"success":false,"data":{"articles":[]}}"#, None).is_err());
        assert!(parse(br#"{"success":true,"data":{}}"#, None).is_err());
        assert!(
            parse(br#"{"success":true,"data":{"articles":[]}}"#, None)
                .unwrap()
                .1
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn only_claims_official_blog_indexes() {
        for value in ["https://qwen.ai/blog", "https://qwen.ai/research/"] {
            assert!(is_blog_url(&Url::parse(value).unwrap()));
        }
        for value in [
            "https://other.ai/blog",
            "https://qwen.ai/blog?id=post",
            "https://qwen.ai/",
        ] {
            assert!(!is_blog_url(&Url::parse(value).unwrap()));
        }
    }
}
