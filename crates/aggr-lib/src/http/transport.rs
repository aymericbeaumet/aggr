use anyhow::{Context, Result};
use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt, stream::BoxStream};
use reqwest::{StatusCode, header::HeaderMap};
use std::time::Duration;
use url::Url;
use wreq_util::{Emulation, Profile};

/// Both transports hand decoded chunks to the same redirect and body-limit logic.
pub(super) struct Received {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub url: Url,
    pub body: BoxStream<'static, Result<Bytes>>,
}

impl Received {
    pub fn ordinary(response: reqwest::Response) -> Self {
        Self {
            status: response.status(),
            headers: response.headers().clone(),
            url: response.url().clone(),
            body: response
                .bytes_stream()
                .map_err(|error| error.without_url().into())
                .boxed(),
        }
    }

    pub fn emulated(response: wreq::Response) -> Result<Self> {
        let url =
            Url::parse(&response.uri().to_string()).context("invalid compatible response URL")?;
        Ok(Self {
            status: response.status(),
            headers: response.headers().clone(),
            url,
            body: response
                .bytes_stream()
                .map_err(|error| error.without_uri().into())
                .boxed(),
        })
    }

    pub fn is_challenge(&self) -> bool {
        self.headers
            .get("cf-mitigated")
            .is_some_and(|value| value == "challenge")
    }
}

pub(super) fn emulated_client(timeout: Duration) -> Result<wreq::Client> {
    wreq::Client::builder()
        .emulation(
            Emulation::builder()
                .profile(Profile::Chrome149)
                .headers(false)
                .build(),
        )
        .user_agent(super::user_agent())
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .redirect(wreq::redirect::Policy::none())
        .gzip(true)
        .brotli(true)
        .build()
        .context("building compatible HTTP transport")
}
