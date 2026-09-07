//! One HTTP client for the whole run: conditional GETs, body caps, retries on transient
//! failures, and a per-host pacing lock.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::header::{
    CONTENT_TYPE, ETAG, HeaderMap, HeaderName, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH,
    LAST_MODIFIED, LOCATION, RETRY_AFTER,
};
use tokio::time::Instant;
use url::Url;

use crate::config::FetchConfig;

pub struct Client {
    inner: reqwest::Client,
    max_body_bytes: usize,
    retries: u32,
    timeout: Duration,
    hosts: HostLimiter,
}

pub struct Request<'a> {
    pub url: &'a Url,
    pub headers: &'a [(String, String)],
    pub etag: Option<&'a str>,
    pub last_modified: Option<&'a str>,
}

impl<'a> Request<'a> {
    #[cfg(test)]
    pub fn get(url: &'a Url) -> Self {
        Self {
            url,
            headers: &[],
            etag: None,
            last_modified: None,
        }
    }
}

#[derive(Debug)]
pub enum Response {
    /// 304: the validators still hold, nothing to read.
    NotModified,
    Ok(Body),
}

#[derive(Debug)]
pub struct Body {
    pub bytes: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// URL after redirects.
    pub final_url: Url,
    pub content_type: Option<String>,
}

impl Body {
    pub fn html_text(&self) -> String {
        decode_html(&self.bytes, self.content_type.as_deref())
    }
}

pub fn source_headers<'a>(
    source: &'a crate::config::Source,
    target: &Url,
) -> &'a [(String, String)] {
    if source
        .engine
        .url()
        .is_some_and(|url| url.origin() == target.origin())
    {
        &source.headers
    } else {
        &[]
    }
}

pub fn is_html_content_type(content_type: Option<&str>) -> bool {
    content_type.is_none_or(|value| {
        matches!(
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "text/html" | "application/xhtml+xml"
        )
    })
}

pub fn decode_html(bytes: &[u8], content_type: Option<&str>) -> String {
    use encoding_rs::{Encoding, UTF_8, WINDOWS_1252};
    if let Some((encoding, length)) = Encoding::for_bom(bytes) {
        return encoding
            .decode_without_bom_handling(&bytes[length..])
            .0
            .into_owned();
    }
    let xhtml = content_type.is_some_and(|value| {
        value.split(';').next().is_some_and(|media_type| {
            media_type
                .trim()
                .eq_ignore_ascii_case("application/xhtml+xml")
        })
    });
    let declared = content_type.and_then(content_type_encoding).or_else(|| {
        if xhtml {
            xml_declaration_encoding(bytes)
        } else {
            html_meta_encoding(bytes).or_else(|| xml_declaration_encoding(bytes))
        }
    });
    let encoding = declared.unwrap_or(if xhtml { UTF_8 } else { WINDOWS_1252 });
    encoding.decode(bytes).0.into_owned()
}

fn content_type_encoding(value: &str) -> Option<&'static encoding_rs::Encoding> {
    value.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        if !name.eq_ignore_ascii_case("charset") {
            return None;
        }
        encoding_rs::Encoding::for_label(value.trim().trim_matches(['\'', '"']).as_bytes())
    })
}

fn html_meta_encoding(bytes: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]);
    let document = scraper::Html::parse_document(&prefix);
    let selector = scraper::Selector::parse("meta").ok()?;
    document.select(&selector).find_map(|meta| {
        if let Some(label) = meta.value().attr("charset") {
            return encoding_rs::Encoding::for_label(label.as_bytes());
        }
        meta.value()
            .attr("http-equiv")
            .filter(|value| value.eq_ignore_ascii_case("content-type"))?;
        content_type_encoding(meta.value().attr("content")?)
    })
}

fn xml_declaration_encoding(bytes: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let prefix = &bytes[..bytes.len().min(1024)];
    if !prefix.starts_with(b"<?xml") {
        return None;
    }
    let end = prefix.windows(2).position(|pair| pair == b"?>")?;
    let declaration = &prefix[..end];
    let marker = b"encoding";
    let start = declaration
        .windows(marker.len())
        .enumerate()
        .find_map(|(index, window)| {
            (window == marker && index > 0 && declaration[index - 1].is_ascii_whitespace())
                .then_some(index)
        })?;
    let mut cursor = start + marker.len();
    while declaration.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    if declaration.get(cursor) != Some(&b'=') {
        return None;
    }
    cursor += 1;
    while declaration.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    let quote = *declaration.get(cursor)?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    cursor += 1;
    let length = declaration[cursor..]
        .iter()
        .position(|byte| *byte == quote)?;
    let label = &declaration[cursor..cursor + length];
    if label.first().is_none_or(|byte| !byte.is_ascii_alphabetic())
        || !label
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
    {
        return None;
    }
    encoding_rs::Encoding::for_label(label)
}

#[cfg(test)]
impl Body {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// reqwest is built with `rustls-no-provider`: a process-wide crypto provider must exist before
/// the first client. `ring` keeps the release matrix free of aws-lc-rs' native toolchain
/// requirements. Installing twice is a harmless error.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Stable identity sent by every aggr-owned HTTP client. Keeping this out of the configuration
/// makes source diagnostics useful and ensures publishers can identify the exact release.
pub fn user_agent() -> String {
    format!(
        "aggr/{} (+https://github.com/aymericbeaumet/aggr)",
        env!("CARGO_PKG_VERSION")
    )
}

impl Client {
    pub(crate) fn max_body_bytes(&self) -> usize {
        self.max_body_bytes
    }

    pub fn new(config: &FetchConfig) -> Result<Self> {
        install_crypto_provider();
        let inner = reqwest::Client::builder()
            .user_agent(user_agent())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(config.timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .gzip(true)
            .brotli(true)
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            inner,
            max_body_bytes: config.max_body_bytes,
            retries: config.retries,
            timeout: Duration::from_secs(config.timeout_secs),
            hosts: HostLimiter::new(Duration::ZERO),
        })
    }

    pub async fn get(&self, request: Request<'_>) -> Result<Response> {
        match tokio::time::timeout(self.timeout, self.get_inner(request)).await {
            Ok(result) => result,
            Err(_) => bail!(
                "request exceeded total timeout of {} seconds",
                self.timeout.as_secs()
            ),
        }
    }

    async fn get_inner(&self, request: Request<'_>) -> Result<Response> {
        if !matches!(request.url.scheme(), "http" | "https") {
            bail!("unsupported request URL scheme");
        }
        let mut headers = HeaderMap::new();
        for (name, value) in request.headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .with_context(|| format!("invalid header name {name:?}"))?;
            let mut value = HeaderValue::from_str(value)
                .with_context(|| format!("invalid value for header {name}"))?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }
        if let Some(etag) = request.etag
            && let Ok(value) = HeaderValue::from_str(etag)
        {
            headers.insert(IF_NONE_MATCH, value);
        }
        if let Some(last_modified) = request.last_modified
            && let Ok(value) = HeaderValue::from_str(last_modified)
        {
            headers.insert(IF_MODIFIED_SINCE, value);
        }

        let mut attempt = 0;
        loop {
            let result = match self.send(request.url, headers.clone()).await {
                Ok(response) => self.read(response).await,
                Err(err) => Err(err),
            };
            match result {
                Ok(response) => return Ok(response),
                Err(err) if attempt < self.retries && is_transient(&err) => {
                    attempt += 1;
                    log::debug!(
                        "{}: retry {attempt} after: {err:#}",
                        request.url.origin().ascii_serialization()
                    );
                    let backoff = Duration::from_millis(
                        500 * 2_u64.saturating_pow(attempt.saturating_sub(1)),
                    );
                    let delay = err
                        .downcast_ref::<HttpStatus>()
                        .and_then(|status| status.1)
                        .unwrap_or(backoff);
                    tokio::time::sleep(delay).await;
                }
                Err(err) => return Err(err),
            }
        }
    }

    async fn send(&self, url: &Url, mut headers: HeaderMap) -> Result<reqwest::Response> {
        let mut url = url.clone();
        for redirects in 0..=10 {
            self.hosts.wait(&url).await;
            let response = self
                .inner
                .get(url.clone())
                .headers(headers.clone())
                .send()
                .await
                .map_err(reqwest::Error::without_url)
                .context("sending request")?;
            if !response.status().is_redirection()
                || response.status() == reqwest::StatusCode::NOT_MODIFIED
            {
                return Ok(response);
            }
            let Some(location) = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
            else {
                return Ok(response);
            };
            if redirects == 10 {
                bail!("too many redirects");
            }
            let mut next = url.join(location).context("invalid redirect destination")?;
            if !matches!(next.scheme(), "http" | "https") {
                bail!("unsupported redirect URL scheme");
            }
            if next.origin() != url.origin() {
                headers.clear();
            }
            // URL origins intentionally ignore userinfo. Never let credentials introduced by a
            // redirect turn into implicit Basic auth or enter persisted final-URL metadata.
            let _ = next.set_username("");
            let _ = next.set_password(None);
            url = next;
        }
        bail!("too many redirects")
    }

    async fn read(&self, mut response: reqwest::Response) -> Result<Response> {
        let status = response.status();
        if status == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(Response::NotModified);
        }
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(retry_after);
            bail!(HttpStatus(status.as_u16(), retry_after));
        }
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let etag = header(ETAG);
        let last_modified = header(LAST_MODIFIED);
        let content_type = header(CONTENT_TYPE);
        let final_url = response.url().clone();

        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(reqwest::Error::without_url)
            .context("reading body")?
        {
            if bytes.len() + chunk.len() > self.max_body_bytes {
                bail!("body exceeds {} bytes", self.max_body_bytes);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Response::Ok(Body {
            bytes,
            etag,
            last_modified,
            final_url,
            content_type,
        }))
    }
}

#[derive(Debug)]
struct HttpStatus(u16, Option<Duration>);

fn retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some(
        (date.with_timezone(&chrono::Utc) - chrono::Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO),
    )
}

impl std::fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {}", self.0)
    }
}

impl std::error::Error for HttpStatus {}

/// HTTP status carried by an error from this client, if the request reached the server.
pub fn status_code(err: &anyhow::Error) -> Option<u16> {
    err.downcast_ref::<HttpStatus>().map(|status| status.0)
}

/// Worth another attempt: 5xx, 429, or a request/body transport failure.
fn is_transient(err: &anyhow::Error) -> bool {
    if let Some(HttpStatus(code, _)) = err.downcast_ref::<HttpStatus>() {
        return *code >= 500 || *code == 429;
    }
    err.downcast_ref::<reqwest::Error>().is_some_and(|e| {
        e.is_timeout() || e.is_connect() || e.is_request() || e.is_body() || e.is_decode()
    })
}

/// Reserves the next slot for a host under a short lock, then sleeps outside it so other hosts
/// are never blocked.
struct HostLimiter {
    delay: Duration,
    next: Mutex<HashMap<String, Instant>>,
}

impl HostLimiter {
    fn new(delay: Duration) -> Self {
        Self {
            delay,
            next: Mutex::new(HashMap::new()),
        }
    }

    async fn wait(&self, url: &Url) {
        if self.delay.is_zero() {
            return;
        }
        let Some(host) = url.host_str() else { return };
        let slot = {
            let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let slot = next.get(host).copied().unwrap_or(now).max(now);
            next.insert(host.to_string(), slot + self.delay);
            slot
        };
        tokio::time::sleep_until(slot).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    #[test]
    fn html_decoding_honors_bom_http_and_meta_charsets() {
        assert_eq!(
            decode_html(b"caf\xe9", Some("text/html; charset=windows-1252")),
            "café"
        );
        assert_eq!(
            decode_html(
                b"\xef\xbb\xbfcaf\xc3\xa9",
                Some("text/html; charset=windows-1252")
            ),
            "café"
        );
        assert!(decode_html(b"<meta charset=windows-1252><p>caf\xe9</p>", None).contains("café"));
        assert!(is_html_content_type(Some("text/html; charset=utf-8")));
        assert!(!is_html_content_type(Some("image/png")));
    }

    #[test]
    fn undeclared_html_uses_the_web_compatible_windows_1252_default() {
        assert_eq!(
            decode_html(b"price: \x8080", Some("text/html")),
            "price: €80"
        );
        assert_eq!(decode_html(b"caf\xe9", None), "café");
    }

    #[test]
    fn xhtml_honors_its_xml_declaration_and_otherwise_defaults_to_utf_8() {
        assert_eq!(
            decode_html(
                b"<?xml version='1.0' encoding = \"windows-1252\"?><p>caf\xe9</p>",
                Some("application/xhtml+xml")
            ),
            "<?xml version='1.0' encoding = \"windows-1252\"?><p>café</p>"
        );
        assert_eq!(
            decode_html(
                b"<?xml version='1.0'?><p>caf\xc3\xa9</p>",
                Some("application/xhtml+xml")
            ),
            "<?xml version='1.0'?><p>café</p>"
        );
    }

    #[tokio::test]
    async fn redirects_drop_all_custom_headers_when_origin_changes() {
        let first = MockServer::start_async().await;
        let second = MockServer::start_async().await;
        let redirect = first
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/start")
                    .header("authorization", "Bearer private")
                    .header("x-secret", "private");
                then.status(302).header("location", second.url("/article"));
            })
            .await;
        let article = second
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/article")
                    .header_missing("authorization")
                    .header_missing("x-secret")
                    .header_missing("cookie");
                then.status(200).body("safe");
            })
            .await;
        let url = Url::parse(&first.url("/start")).unwrap();
        let headers = vec![
            ("Authorization".into(), "Bearer private".into()),
            ("X-Secret".into(), "private".into()),
            ("Cookie".into(), "session=private".into()),
        ];
        let response = client()
            .get(Request {
                url: &url,
                headers: &headers,
                etag: None,
                last_modified: None,
            })
            .await
            .unwrap();
        assert!(matches!(response, Response::Ok(_)));
        redirect.assert_async().await;
        article.assert_async().await;
    }

    #[tokio::test]
    async fn same_origin_redirects_strip_injected_url_credentials() {
        let server = MockServer::start_async().await;
        let mut target = Url::parse(&server.url("/article")).unwrap();
        target.set_username("redirect-user").unwrap();
        target.set_password(Some("redirect-secret")).unwrap();
        let redirect = server
            .mock_async(|when, then| {
                when.method(GET).path("/start");
                then.status(302).header("location", target.as_str());
            })
            .await;
        let article = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/article")
                    .header_missing("authorization");
                then.status(200).body("safe");
            })
            .await;
        let url = Url::parse(&server.url("/start")).unwrap();
        let Response::Ok(body) = client().get(Request::get(&url)).await.unwrap() else {
            panic!("expected a body");
        };
        assert!(body.final_url.username().is_empty());
        assert!(body.final_url.password().is_none());
        redirect.assert_async().await;
        article.assert_async().await;
    }

    #[tokio::test]
    async fn connection_errors_do_not_expose_private_request_urls() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let url = Url::parse(&format!(
            "http://{address}/private?token=private-test-secret"
        ))
        .unwrap();
        let client = Client::new(&FetchConfig {
            retries: 0,
            ..Default::default()
        })
        .unwrap();
        let error = client.get(Request::get(&url)).await.unwrap_err();
        assert!(!format!("{error:#}").contains("private-test-secret"));
    }

    #[tokio::test]
    async fn retry_after_cannot_outlive_the_total_request_budget() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/busy");
                then.status(429).header("retry-after", "30");
            })
            .await;
        let client = Client::new(&FetchConfig {
            timeout_secs: 1,
            retries: 2,
            ..Default::default()
        })
        .unwrap();
        let started = Instant::now();
        let error = client
            .get(Request::get(&Url::parse(&server.url("/busy")).unwrap()))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("total timeout"));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(mock.calls_async().await, 1);
    }

    fn client() -> Client {
        Client::new(&FetchConfig {
            retries: 1,
            max_body_bytes: 64,
            ..Default::default()
        })
        .unwrap()
    }

    #[tokio::test]
    async fn conditional_get_round_trips_validators() {
        let server = MockServer::start_async().await;
        let first = server
            .mock_async(|when, then| {
                when.method(GET).path("/feed");
                then.status(200)
                    .header("etag", "\"v1\"")
                    .header("last-modified", "Tue, 01 Sep 2026 00:00:00 GMT")
                    .body("hello");
            })
            .await;
        let url = Url::parse(&server.url("/feed")).unwrap();
        let Response::Ok(body) = client().get(Request::get(&url)).await.unwrap() else {
            panic!("expected a body");
        };
        assert_eq!(body.text(), "hello");
        assert_eq!(body.etag.as_deref(), Some("\"v1\""));
        first.assert_async().await;
        first.delete_async().await;

        let not_modified = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/feed")
                    .header("if-none-match", "\"v1\"");
                then.status(304);
            })
            .await;
        let response = client()
            .get(Request {
                etag: Some("\"v1\""),
                last_modified: body.last_modified.as_deref(),
                ..Request::get(&url)
            })
            .await
            .unwrap();
        assert!(matches!(response, Response::NotModified));
        not_modified.assert_async().await;
    }

    #[tokio::test]
    async fn sends_custom_headers() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/api")
                    .header("authorization", "Bearer t");
                then.status(200).body("[]");
            })
            .await;
        let url = Url::parse(&server.url("/api")).unwrap();
        let headers = vec![("Authorization".to_string(), "Bearer t".to_string())];
        client()
            .get(Request {
                headers: &headers,
                ..Request::get(&url)
            })
            .await
            .unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn identifies_aggr_and_its_version() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/identity")
                    .header("user-agent", user_agent());
                then.status(200).body("ok");
            })
            .await;
        let url = Url::parse(&server.url("/identity")).unwrap();
        client().get(Request::get(&url)).await.unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn caps_body_size() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/big");
                then.status(200).body("x".repeat(65));
            })
            .await;
        let url = Url::parse(&server.url("/big")).unwrap();
        let err = client().get(Request::get(&url)).await.unwrap_err();
        assert!(err.to_string().contains("exceeds 64 bytes"), "{err}");
    }

    #[tokio::test]
    async fn retries_server_errors_then_fails() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/flaky");
                then.status(503);
            })
            .await;
        let url = Url::parse(&server.url("/flaky")).unwrap();
        let err = client().get(Request::get(&url)).await.unwrap_err();
        assert!(err.to_string().contains("HTTP 503"), "{err}");
        assert_eq!(mock.calls_async().await, 2, "one retry");
    }

    async fn interrupted_body_server(
        responses: Vec<&'static [u8]>,
    ) -> (Url, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!(
            "http://{}/article",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let task = tokio::spawn(async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = vec![];
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await.unwrap());
                }
                stream.write_all(response).await.unwrap();
                stream.shutdown().await.unwrap();
            }
        });
        (url, task)
    }

    #[tokio::test]
    async fn retries_interrupted_and_undecodable_bodies_without_keeping_partial_content() {
        for first in [
            &b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\npartial"[..],
            &b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad"[..],
        ] {
            let (url, server) = interrupted_body_server(vec![
                first,
                b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\ncomplete",
            ]).await;
            let result = client().get(Request::get(&url)).await;
            server.abort();
            let Response::Ok(body) = result.unwrap() else {
                panic!("expected a body");
            };
            assert_eq!(body.text(), "complete");
        }
    }

    #[tokio::test]
    async fn interrupted_body_retries_are_bounded() {
        let (url, server) = interrupted_body_server(vec![
            b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\npartial",
            b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\npartial",
        ])
        .await;
        let error = client().get(Request::get(&url)).await.unwrap_err();
        assert!(error.to_string().contains("reading body"), "{error:#}");
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn does_not_retry_client_errors() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/gone");
                then.status(404);
            })
            .await;
        let url = Url::parse(&server.url("/gone")).unwrap();
        let err = client().get(Request::get(&url)).await.unwrap_err();
        assert_eq!(status_code(&err), Some(404));
        assert_eq!(mock.calls_async().await, 1);
    }

    // Paused time: the clock only moves when the limiter sleeps, so no runner is too slow.
    #[tokio::test(start_paused = true)]
    async fn host_limiter_spaces_requests() {
        let limiter = HostLimiter::new(Duration::from_millis(30));
        let url = Url::parse("https://example.com/a").unwrap();
        let other = Url::parse("https://other.example/a").unwrap();
        let start = Instant::now();
        limiter.wait(&url).await;
        limiter.wait(&other).await;
        assert!(
            start.elapsed() < Duration::from_millis(25),
            "different hosts do not wait"
        );
        limiter.wait(&url).await;
        assert!(
            start.elapsed() >= Duration::from_millis(30),
            "same host waits"
        );
    }
}
