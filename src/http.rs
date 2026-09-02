//! One HTTP client for the whole run: conditional GETs, body caps, retries on transient
//! failures, and a per-host pacing lock.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::header::{
    ETAG, HeaderMap, HeaderName, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
};
use tokio::time::Instant;
use url::Url;

use crate::config::FetchConfig;

pub struct Client {
    inner: reqwest::Client,
    max_body_bytes: usize,
    retries: u32,
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

impl Client {
    pub fn new(config: &FetchConfig) -> Result<Self> {
        install_crypto_provider();
        let inner = reqwest::Client::builder()
            .user_agent(config.user_agent())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(config.timeout_secs))
            .redirect(reqwest::redirect::Policy::limited(10))
            .gzip(true)
            .brotli(true)
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            inner,
            max_body_bytes: config.max_body_bytes,
            retries: config.retries,
            hosts: HostLimiter::new(Duration::ZERO),
        })
    }

    pub fn raw(&self) -> &reqwest::Client {
        &self.inner
    }

    pub async fn get(&self, request: Request<'_>) -> Result<Response> {
        let mut headers = HeaderMap::new();
        for (name, value) in request.headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .with_context(|| format!("invalid header name {name:?}"))?;
            let value = HeaderValue::from_str(value)
                .with_context(|| format!("invalid value for header {name}"))?;
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
            self.hosts.wait(request.url).await;
            let result = match self
                .inner
                .get(request.url.clone())
                .headers(headers.clone())
                .send()
                .await
                .context("sending request")
            {
                Ok(response) => self.read(response).await,
                Err(err) => Err(err),
            };
            match result {
                Ok(response) => return Ok(response),
                Err(err) if attempt < self.retries && is_transient(&err) => {
                    attempt += 1;
                    log::debug!("{}: retry {attempt} after: {err:#}", request.url);
                    tokio::time::sleep(Duration::from_millis(500 * u64::from(attempt))).await;
                }
                Err(err) => return Err(err),
            }
        }
    }

    async fn read(&self, mut response: reqwest::Response) -> Result<Response> {
        let status = response.status();
        if status == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(Response::NotModified);
        }
        if !status.is_success() {
            bail!(HttpStatus(status.as_u16()));
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
        let final_url = response.url().clone();

        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.context("reading body")? {
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
        }))
    }
}

#[derive(Debug)]
struct HttpStatus(u16);

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

/// Worth another attempt: 5xx, 429, or a connection/timeout failure.
fn is_transient(err: &anyhow::Error) -> bool {
    if let Some(HttpStatus(code)) = err.downcast_ref::<HttpStatus>() {
        return *code >= 500 || *code == 429;
    }
    err.downcast_ref::<reqwest::Error>()
        .is_some_and(|e| e.is_timeout() || e.is_connect() || e.is_request())
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
