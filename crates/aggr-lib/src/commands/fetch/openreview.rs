//! Public paper metadata, independent of OpenReview's challenged web application.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use url::Url;

use crate::{cache::ArticleCache, config::Source, http, model::RawItem};

pub(super) fn note_id(url: &Url) -> Option<String> {
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("openreview.net" | "www.openreview.net")
        )
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let forum = if url.path() == "/challenge" {
        let redirects: Vec<_> = url
            .query_pairs()
            .filter(|(key, _)| key == "redirect")
            .collect();
        let [(_, redirect)] = redirects.as_slice() else {
            return None;
        };
        let target = url.join(redirect).ok()?;
        if target.origin() != url.origin() || target.path() != "/forum" {
            return None;
        }
        target
    } else {
        url.clone()
    };
    if forum.path() != "/forum" {
        return None;
    }
    let ids: Vec<_> = forum.query_pairs().filter(|(key, _)| key == "id").collect();
    let [(_, id)] = ids.as_slice() else {
        return None;
    };
    (!id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then(|| id.to_string())
}

pub(super) async fn enrich(
    raw: &RawItem,
    id: &str,
    source: &Source,
    client: &http::Client,
    cache: &ArticleCache,
) -> Result<RawItem> {
    let endpoint = Url::parse("https://api2.openreview.net/notes/search")?;
    enrich_at(raw, id, source, client, cache, endpoint).await
}

async fn enrich_at(
    raw: &RawItem,
    id: &str,
    source: &Source,
    client: &http::Client,
    cache: &ArticleCache,
    mut endpoint: Url,
) -> Result<RawItem> {
    let title: String = crate::content::html_to_text(&raw.title)
        .chars()
        .take(512)
        .collect();
    if title.trim().is_empty() {
        bail!("OpenReview paper discovery needs a title");
    }
    endpoint
        .query_pairs_mut()
        .append_pair("query", &title)
        .append_pair("content", "title")
        .append_pair("limit", "20");
    let headers = http::source_headers(source, &endpoint);
    let cached = cache.load(&endpoint, headers)?;
    let response = client
        .get(http::Request {
            url: &endpoint,
            headers,
            etag: cached.as_ref().and_then(|body| body.etag.as_deref()),
            last_modified: cached
                .as_ref()
                .and_then(|body| body.last_modified.as_deref()),
        })
        .await;
    let (mut enriched, bytes) = match response {
        Ok(http::Response::Ok(body)) => {
            let enriched = from_response(raw, id, &body.bytes)?;
            cache.store(&endpoint, headers, &body)?;
            (enriched, body.bytes)
        }
        Ok(http::Response::NotModified) => {
            let cached = cached.context("OpenReview returned 304 without cached metadata")?;
            (from_response(raw, id, &cached.bytes)?, cached.bytes)
        }
        Err(error) => {
            let Some(cached) = cached else {
                return Err(error)
                    .context("OpenReview public metadata unavailable; retaining feed content");
            };
            (from_response(raw, id, &cached.bytes)?, cached.bytes)
        }
    };
    let data: Value = serde_json::from_slice(&bytes)?;
    if let Some(arxiv_id) = arxiv_alternate(&data, id) {
        let base = Url::parse("https://arxiv.org/")?;
        if let Err(error) =
            enrich_arxiv(&mut enriched, &arxiv_id, &base, source, client, cache).await
        {
            log::debug!(
                "OpenReview alternate paper unavailable; preserving public abstract: {error:#}"
            );
        }
    }
    Ok(enriched)
}

fn public_submission(note: &Value) -> bool {
    note.get("replyto").is_none_or(Value::is_null)
        && note
            .get("readers")
            .and_then(Value::as_array)
            .is_some_and(|readers| {
                readers
                    .iter()
                    .any(|reader| reader.as_str() == Some("everyone"))
            })
}

fn arxiv_alternate(data: &Value, id: &str) -> Option<String> {
    let notes = data.get("notes")?.as_array()?;
    let note = notes
        .iter()
        .find(|note| note.get("id").and_then(Value::as_str) == Some(id))?;
    let title = text(note, "title")?;
    let paperhash = text(note, "paperhash")?;
    let authors = field(note, "authors")?.as_array()?;
    if authors.is_empty()
        || authors
            .iter()
            .any(|author| author.as_str().is_none_or(|name| name.trim().is_empty()))
    {
        return None;
    }
    let mut ids = std::collections::BTreeSet::new();
    for other in notes.iter().filter(|other| {
        public_submission(other)
            && text(other, "title") == Some(title)
            && text(other, "paperhash") == Some(paperhash)
            && field(other, "authors").and_then(Value::as_array) == Some(authors)
    }) {
        if let Some(id) = text(other, "html").and_then(arxiv_id) {
            ids.insert(id);
        }
    }
    (ids.len() == 1).then(|| ids.into_iter().next()).flatten()
}

fn arxiv_id(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let id = match url.host_str()? {
        "doi.org" => url.path().strip_prefix("/10.48550/arXiv.")?,
        "arxiv.org" => url.path().strip_prefix("/abs/")?,
        _ => return None,
    };
    let plain = id.split_once('v').map_or(id, |(plain, _)| plain);
    let (month, number) = plain.split_once('.')?;
    if month.len() != 4
        || !(4..=5).contains(&number.len())
        || !month
            .bytes()
            .chain(number.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    if id != plain {
        let version = id.strip_prefix(plain)?.strip_prefix('v')?;
        if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    Some(id.to_owned())
}

async fn cached_response(
    url: &Url,
    source: &Source,
    client: &http::Client,
    cache: &ArticleCache,
) -> Result<crate::cache::ArticleResponse> {
    let headers = http::source_headers(source, url);
    let cached = cache.load(url, headers)?;
    match client
        .get(http::Request {
            url,
            headers,
            etag: cached.as_ref().and_then(|body| body.etag.as_deref()),
            last_modified: cached
                .as_ref()
                .and_then(|body| body.last_modified.as_deref()),
        })
        .await
    {
        Ok(http::Response::Ok(body)) => cache.store(url, headers, &body),
        Ok(http::Response::NotModified) => {
            cached.context("paper returned 304 without a cached response")
        }
        Err(error) => cached.ok_or(error),
    }
}

async fn enrich_arxiv(
    raw: &mut RawItem,
    id: &str,
    base: &Url,
    source: &Source,
    client: &http::Client,
    cache: &ArticleCache,
) -> Result<()> {
    let html_url = base.join(&format!("html/{id}"))?;
    let extracted = async {
        let response = cached_response(&html_url, source, client, cache).await?;
        if !http::is_html_content_type(response.content_type.as_deref()) {
            bail!("arXiv alternate is not HTML")
        }
        let key = response.extraction_key();
        let extracted = if let Some(extracted) = cache.extracted(&key, &response.final_url)? {
            extracted
        } else {
            let extracted = crate::content::extract_article_async(
                response.html_text(),
                response.final_url.clone(),
            )
            .await?;
            cache.store_extracted(&key, &response.final_url, &extracted)?;
            extracted
        };
        // The retained item's identity remains OpenReview, so resolve relative paper assets here.
        Ok::<_, anyhow::Error>(crate::content::sanitize(
            &crate::content::storage_html(&readable_math(&extracted.html), usize::MAX).0,
            Some(&response.final_url),
        ))
    }
    .await;
    match extracted {
        Ok(html) => {
            raw.content_html = Some(format!(
                "<p><a href=\"{}\">Read the authors’ arXiv preprint</a> · <a href=\"{}\">PDF with original typesetting</a></p>{html}",
                escape(html_url.as_str()),
                escape(base.join(&format!("pdf/{id}.pdf"))?.as_str())
            ));
            Ok(())
        }
        Err(error) => {
            log::debug!("arXiv HTML unavailable; checking its public PDF: {error:#}");
            let pdf = base.join(&format!("pdf/{id}.pdf"))?;
            let response = cached_response(&pdf, source, client, cache).await?;
            if !response.bytes.starts_with(b"%PDF-")
                || response
                    .content_type
                    .as_deref()
                    .is_none_or(|mime| mime.split(';').next() != Some("application/pdf"))
            {
                bail!("arXiv alternate did not return a PDF");
            }
            raw.extra.insert("document_url".into(), pdf.as_str().into());
            Ok(())
        }
    }
}

fn readable_math(html: &str) -> String {
    static MATH: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = MATH.get_or_init(|| {
        regex::Regex::new(r"(?is)<math\b[^>]*>.*?</math\s*>").expect("valid MathML pattern")
    });
    pattern
        .replace_all(html, |captures: &regex::Captures<'_>| {
            let fragment = scraper::Html::parse_fragment(&captures[0]);
            let formula = fragment
                .tree
                .nodes()
                .filter_map(scraper::ElementRef::wrap)
                .find(|element| element.value().name() == "math")
                .and_then(|element| element.value().attr("alttext"));
            formula.map_or_else(
                || captures[0].to_owned(),
                |formula| format!("<code>{}</code>", escape(formula)),
            )
        })
        .into_owned()
}

fn field<'a>(note: &'a Value, key: &str) -> Option<&'a Value> {
    let value = note.get("content")?.get(key)?;
    Some(value.get("value").unwrap_or(value))
}

fn text<'a>(note: &'a Value, key: &str) -> Option<&'a str> {
    field(note, key)?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn from_response(raw: &RawItem, id: &str, bytes: &[u8]) -> Result<RawItem> {
    let response: Value =
        serde_json::from_slice(bytes).context("decoding OpenReview paper metadata")?;
    let notes = response
        .get("notes")
        .and_then(Value::as_array)
        .context("OpenReview response has no notes")?;
    let mut matches = notes
        .iter()
        .filter(|note| note.get("id").and_then(Value::as_str) == Some(id));
    let note = matches
        .next()
        .context("OpenReview public search did not return the requested paper ID")?;
    if matches.next().is_some() || !public_submission(note) {
        bail!("OpenReview paper is ambiguous or not a public submission");
    }
    let abstract_text =
        text(note, "abstract").context("OpenReview paper has no public abstract")?;
    let mut html = format!("<h2>Abstract</h2><p>{}</p>", escape(abstract_text));
    if let Some(summary) = text(note, "lay_summary") {
        html.push_str(&format!(
            "<h2>Plain-language summary</h2><p>{}</p>",
            escape(summary)
        ));
    }
    if let Some(pdf) = text(note, "pdf").and_then(pdf_url) {
        html.push_str(&format!(
            "<p><a href=\"{}\">Read the paper on OpenReview (PDF)</a></p>",
            escape(pdf.as_str())
        ));
    }
    let mut enriched = raw.clone();
    enriched.content_html = Some(html);
    if enriched.authors.is_empty() {
        enriched.authors = field(note, "authors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
    }
    if enriched
        .summary
        .as_deref()
        .is_none_or(|summary| summary.trim().is_empty())
    {
        enriched.summary = Some(abstract_text.to_owned());
    }
    Ok(enriched)
}

fn pdf_url(value: &str) -> Option<Url> {
    let url = Url::parse("https://openreview.net/")
        .ok()?
        .join(value)
        .ok()?;
    (url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some("openreview.net" | "api2.openreview.net")
        )
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.path().starts_with("/pdf/")
        && url.path().ends_with(".pdf")
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn response() -> Value {
        json!({"notes":[{"id":"requested", "readers":["everyone"], "content": {
            "title":{"value":"A paper"}, "abstract":{"value":"A <script> must stay text & readable."},
            "lay_summary":{"value":"An accessible explanation."}, "authors":{"value":["Researcher"]},
            "pdf":{"value":"/pdf/abc.pdf"}
        }}]})
    }

    #[test]
    fn recognizes_only_safe_forum_and_same_origin_challenge_urls() {
        for url in [
            "https://openreview.net/forum?id=requested#discussion",
            "https://openreview.net/challenge?redirect=%2Fforum%3Fid%3Drequested",
        ] {
            assert_eq!(
                note_id(&Url::parse(url).unwrap()).as_deref(),
                Some("requested")
            );
        }
        for url in [
            "https://evil.test/forum?id=requested",
            "https://openreview.net/challenge?redirect=https://evil.test/forum?id=requested",
            "https://openreview.net/forum?id=a&id=b",
            "https://user@openreview.net/forum?id=requested",
            "https://openreview.net/forum?id=%2Fetc",
            "https://openreview.net/pdf?id=requested",
        ] {
            assert_eq!(note_id(&Url::parse(url).unwrap()), None, "{url}");
        }
    }

    #[test]
    fn exact_public_identity_is_required_and_publisher_text_is_escaped() {
        let raw = RawItem {
            title: "Feed title".into(),
            link: "https://openreview.net/forum?id=requested".into(),
            ..Default::default()
        };
        let enriched =
            from_response(&raw, "requested", &serde_json::to_vec(&response()).unwrap()).unwrap();
        assert_eq!(enriched.link, raw.link);
        assert_eq!(enriched.title, raw.title);
        assert_eq!(enriched.authors, ["Researcher"]);
        let html = enriched.content_html.unwrap();
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("Plain-language summary"));
        assert!(html.contains("https://openreview.net/pdf/abc.pdf"));
        assert!(
            !enriched.extra.contains_key("document_url"),
            "unverified PDFs must not cover readable text"
        );
        assert!(
            from_response(&raw, "different", &serde_json::to_vec(&response()).unwrap()).is_err()
        );
        let mut private = response();
        private["notes"][0]["readers"] = json!(["authors"]);
        assert!(from_response(&raw, "requested", &serde_json::to_vec(&private).unwrap()).is_err());
        let mut reply = response();
        reply["notes"][0]["replyto"] = json!("parent");
        assert!(from_response(&raw, "requested", &serde_json::to_vec(&reply).unwrap()).is_err());
    }

    #[test]
    fn untrusted_pdf_urls_are_not_used() {
        for value in [
            "https://evil.test/paper.pdf",
            "//evil.test/paper.pdf",
            "javascript:alert(1)",
            "https://user@openreview.net/pdf/file.pdf",
        ] {
            assert!(pdf_url(value).is_none());
        }
    }

    #[test]
    fn math_has_one_readable_formula_instead_of_duplicated_annotations() {
        let html = r#"Before <math alttext="x &lt; y"><semantics><mi>x</mi><mo>&lt;</mo><mi>y</mi><annotation encoding="application/x-tex">x &lt; y</annotation></semantics></math> after."#;
        assert_eq!(readable_math(html), "Before <code>x &lt; y</code> after.");
    }

    #[test]
    fn arxiv_alternate_requires_exact_paper_identity_and_all_authors() {
        let mut data = response();
        data["notes"][0]["content"]["paperhash"] = json!({"value":"researcher|a_paper"});
        let mut alternate = data["notes"][0].clone();
        alternate["id"] = json!("another-venue");
        alternate["content"]["html"] = json!({"value":"https://doi.org/10.48550/arXiv.2511.06148"});
        data["notes"].as_array_mut().unwrap().push(alternate);
        assert_eq!(
            arxiv_alternate(&data, "requested").as_deref(),
            Some("2511.06148")
        );
        data["notes"][1]["content"]["authors"] = json!({"value":["Someone else"]});
        assert_eq!(arxiv_alternate(&data, "requested"), None);
        data["notes"][1]["content"]["authors"] = json!({"value":["Researcher"]});
        data["notes"][1]["content"]["title"] = json!({"value":"A different paper"});
        assert_eq!(arxiv_alternate(&data, "requested"), None);
        data["notes"][1]["content"]["title"] = json!({"value":"A paper"});
        data["notes"][1]["readers"] = json!(["authors"]);
        assert_eq!(arxiv_alternate(&data, "requested"), None);
    }

    #[tokio::test]
    async fn metadata_uses_conditional_cache_without_forwarding_feed_credentials() {
        crate::http::install_crypto_provider();
        let server = httpmock::MockServer::start();
        let mut found = server.mock(|when, then| {
            when.path("/notes/search")
                .query_param("query", "A paper")
                .query_param("content", "title")
                .query_param("limit", "20")
                .header_missing("authorization")
                .header_missing("cookie");
            then.status(200)
                .header("content-type", "application/json")
                .header("etag", "paper-v1")
                .json_body(response());
        });
        let mut source = super::super::tests::source();
        source.headers = vec![
            ("authorization".into(), "Bearer private-feed".into()),
            ("cookie".into(), "session=private".into()),
        ];
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let cache = ArticleCache::new(directory.path());
        let raw = RawItem {
            title: "A paper".into(),
            link: "https://openreview.net/forum?id=requested".into(),
            ..Default::default()
        };
        let endpoint = Url::parse(&server.url("/notes/search")).unwrap();
        let first = enrich_at(
            &raw,
            "requested",
            &source,
            &client,
            &cache,
            endpoint.clone(),
        )
        .await
        .unwrap();
        found.assert_calls(1);
        found.delete();
        let mut unchanged = server.mock(|when, then| {
            when.path("/notes/search")
                .header("if-none-match", "paper-v1");
            then.status(304);
        });
        assert_eq!(
            enrich_at(
                &raw,
                "requested",
                &source,
                &client,
                &cache,
                endpoint.clone()
            )
            .await
            .unwrap(),
            first
        );
        unchanged.assert_calls(1);
        unchanged.delete();
        let unavailable = server.mock(|when, then| {
            when.path("/notes/search");
            then.status(403);
        });
        assert_eq!(
            enrich_at(&raw, "requested", &source, &client, &cache, endpoint)
                .await
                .unwrap(),
            first
        );
        unavailable.assert_calls(1);
    }

    #[tokio::test]
    async fn alternate_html_keeps_full_text_and_resolves_assets_without_requesting_pdf() {
        crate::http::install_crypto_provider();
        let server = httpmock::MockServer::start();
        let page = format!(
            "<html><body><article><h1>Research</h1><p>{}</p><img src=\"figure.png\"><h2>Conclusion</h2><p>Ending retained.</p></article></body></html>",
            "Evidence and analysis from the authors. ".repeat(80)
        );
        let fetched_html = server.mock(|when, then| {
            when.path("/html/2511.06148")
                .header_missing("authorization");
            then.status(200)
                .header("content-type", "text/html")
                .body(&page);
        });
        let pdf = server.mock(|when, then| {
            when.path("/pdf/2511.06148.pdf");
            then.status(500);
        });
        let directory = tempfile::tempdir().unwrap();
        let cache = ArticleCache::new(directory.path());
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let mut raw = RawItem {
            link: "https://openreview.net/forum?id=requested".into(),
            ..Default::default()
        };
        enrich_arxiv(
            &mut raw,
            "2511.06148",
            &Url::parse(&server.base_url()).unwrap(),
            &super::super::tests::source(),
            &client,
            &cache,
        )
        .await
        .unwrap();
        let html = raw.content_html.as_deref().unwrap();
        assert!(html.contains("Ending retained."));
        assert!(html.contains(&server.url("/html/figure.png")));
        assert_eq!(raw.link, "https://openreview.net/forum?id=requested");
        assert!(!raw.extra.contains_key("document_url"));
        pdf.assert_calls(0);
        fetched_html.assert_calls(1);
        assert!(
            crate::content::html_to_text(raw.content_html.as_deref().unwrap())
                .split_whitespace()
                .count()
                > 400
        );
    }

    #[tokio::test]
    async fn unavailable_html_requires_real_pdf_bytes_before_embedding() {
        crate::http::install_crypto_provider();
        let server = httpmock::MockServer::start();
        let html = server.mock(|when, then| {
            when.path("/html/2511.06148");
            then.status(404);
        });
        let mut denied = server.mock(|when, then| {
            when.path("/pdf/2511.06148.pdf");
            then.status(403);
        });
        let directory = tempfile::tempdir().unwrap();
        let cache = ArticleCache::new(directory.path());
        let client = http::Client::new(&crate::config::FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let mut raw = RawItem {
            content_html: Some("<p>Usable public abstract</p>".into()),
            ..Default::default()
        };
        let base = Url::parse(&server.base_url()).unwrap();
        assert!(
            enrich_arxiv(
                &mut raw,
                "2511.06148",
                &base,
                &super::super::tests::source(),
                &client,
                &cache
            )
            .await
            .is_err()
        );
        assert_eq!(
            raw.content_html.as_deref(),
            Some("<p>Usable public abstract</p>")
        );
        assert!(!raw.extra.contains_key("document_url"));
        denied.assert_calls(1);
        denied.delete();
        let pdf = server.mock(|when, then| {
            when.path("/pdf/2511.06148.pdf");
            then.status(200)
                .header("content-type", "application/pdf")
                .body("%PDF-1.7\nfixture");
        });
        enrich_arxiv(
            &mut raw,
            "2511.06148",
            &base,
            &super::super::tests::source(),
            &client,
            &cache,
        )
        .await
        .unwrap();
        assert_eq!(
            raw.extra["document_url"].as_str(),
            Some(server.url("/pdf/2511.06148.pdf").as_str())
        );
        html.assert_calls(2);
        pdf.assert_calls(1);
    }
}
