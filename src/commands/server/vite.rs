//! Vite's backend integration changes development HTTP responses only.

use anyhow::{Context as _, Result, ensure};
use url::{Host, Url};

const ASSETS_START: &str = "<!-- aggr:client-assets -->";
const ASSETS_END: &str = "<!-- /aggr:client-assets -->";
const ENTRY_START: &str = "<!-- aggr:client-entry -->";
const ENTRY_END: &str = "<!-- /aggr:client-entry -->";

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

    pub(super) fn inject(&self, body: &[u8]) -> Vec<u8> {
        let Ok(html) = std::str::from_utf8(body) else {
            return body.to_vec();
        };
        let (Some(assets), Some(entry)) = (
            region(html, ASSETS_START, ASSETS_END),
            region(html, ENTRY_START, ENTRY_END),
        ) else {
            return body.to_vec();
        };
        if assets.end > entry.start {
            return body.to_vec();
        }
        let scripts = format!(
            "<script type=\"module\" src=\"{0}/@vite/client\"></script>\n<script type=\"module\" src=\"{0}/src/main.ts\"></script>",
            self.origin,
        );
        let mut injected = Vec::with_capacity(body.len() + scripts.len());
        injected.extend_from_slice(&body[..assets.start]);
        injected.extend_from_slice(&body[assets.end..entry.start]);
        injected.extend_from_slice(scripts.as_bytes());
        injected.extend_from_slice(&body[entry.end..]);
        injected
    }
}

fn region(html: &str, start: &str, end: &str) -> Option<std::ops::Range<usize>> {
    let from = html.find(start)?;
    let content = from + start.len();
    let to = content + html[content..].find(end)? + end.len();
    Some(from..to)
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

    #[test]
    fn replaces_only_the_compiled_client_and_keeps_the_model_and_bootstrap() {
        let body = br#"<!doctype html><html><head>
<link rel="stylesheet" href="/repo/assets/style-main.css">
<!-- aggr:client-assets -->
<link rel="modulepreload" href="/repo/assets/app/shared-compiled.js">
<link rel="stylesheet" href="/repo/assets/app/app-compiled.css">
<!-- /aggr:client-assets -->
<script src="/repo/assets/app/bootstrap.js"></script>
<script id="aggr-page" type="application/json">{"base":"/repo/"}</script>
</head><body><main>Article</main>
<!-- aggr:client-entry -->
<script type="module" src="/repo/assets/app/app-compiled.js"></script>
<!-- /aggr:client-entry -->
</body></html>"#;
        let client = Client::parse("http://127.0.0.1:5173").unwrap();
        let injected = client.inject(body);
        let html = std::str::from_utf8(&injected).unwrap();
        assert!(html.contains("href=\"/repo/assets/style-main.css\""));
        assert!(html.contains("src=\"/repo/assets/app/bootstrap.js\""));
        assert!(html.contains("{\"base\":\"/repo/\"}"));
        assert!(html.contains("<main>Article</main>"));
        assert!(!html.contains("compiled"));
        assert_eq!(
            html.matches("http://127.0.0.1:5173/@vite/client").count(),
            1
        );
        assert_eq!(html.matches("http://127.0.0.1:5173/src/main.ts").count(), 1);
        assert_eq!(html.matches("type=\"module\"").count(), 2);
        assert_eq!(client.inject(&injected), injected);
        assert!(
            std::str::from_utf8(body)
                .unwrap()
                .contains("app-compiled.js")
        );
    }

    #[test]
    fn leaves_loading_pages_and_incomplete_markers_unchanged() {
        let client = Client::parse("http://localhost:5173").unwrap();
        for body in [
            "<p>Preparing the archive</p>",
            "<!-- aggr:client-assets -->incomplete",
            "<!-- aggr:client-assets --><!-- /aggr:client-assets -->",
            "<!-- aggr:client-entry --><!-- /aggr:client-entry -->",
        ] {
            assert_eq!(client.inject(body.as_bytes()), body.as_bytes());
        }
        assert_eq!(client.inject(&[0xff]), [0xff]);
    }
}
