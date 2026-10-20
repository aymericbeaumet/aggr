//! Validate the loopback origin used by the development front door.

use anyhow::{Context as _, Result, ensure};
use url::{Host, Url};

#[derive(Clone)]
pub(super) struct Client {
    origin: String,
}

impl Client {
    pub(super) fn origin(&self) -> &str {
        &self.origin
    }

    pub(super) fn from_env() -> Result<Option<Self>> {
        match std::env::var("AGGR_VITE_URL") {
            Ok(value) => Self::parse(&value).map(Some),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(error) => Err(error).context("reading AGGR_VITE_URL"),
        }
    }

    fn parse(value: &str) -> Result<Self> {
        let url = Url::parse(value).context("parsing AGGR_VITE_URL")?;
        let loopback = match url.host() {
            Some(Host::Domain(host)) => host == "localhost",
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
        let authority = value.split_once("://").map(|(_, rest)| rest);
        let origin_only = authority.is_some_and(|rest| {
            let (authority, suffix) = rest
                .find(['/', '?', '#'])
                .map_or((rest, ""), |at| (&rest[..at], &rest[at..]));
            !authority.contains('@') && matches!(suffix, "" | "/")
        });
        ensure!(
            url.scheme() == "http"
                && loopback
                && origin_only
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && !value.contains('\\')
                && !value.chars().any(char::is_whitespace),
            "AGGR_VITE_URL must be a loopback HTTP origin (for example http://127.0.0.1:5173)"
        );
        Ok(Self {
            origin: url.origin().ascii_serialization(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_loopback_http_origins() {
        for origin in [
            "http://localhost:5173",
            "http://127.0.0.1:5173/",
            "http://127.0.0.2:5173",
            "http://[::1]:5173",
        ] {
            let client = Client::parse(origin).unwrap();
            assert_eq!(client.origin, origin.trim_end_matches('/'));
        }
        for value in [
            "",
            "https://localhost:5173",
            "http://example.com:5173",
            "http://localhost.example.com:5173",
            "http://0.0.0.0:5173",
            "http://[::]:5173",
            "http://user@localhost:5173",
            "http://@localhost:5173",
            "http://localhost:5173/client",
            "http://localhost:5173/client/..",
            "http://localhost:5173/?query",
            "http://localhost:5173/#fragment",
            "http://localhost:5173\\client",
            " http://localhost:5173",
        ] {
            assert!(Client::parse(value).is_err(), "accepted {value:?}");
        }
    }
}
