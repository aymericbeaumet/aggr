use anyhow::{Context as _, Result, bail};
use regex::Regex;
use url::Url;

pub(super) struct ClientDev {
    origin: String,
    script: Regex,
    stylesheet: Regex,
}

impl ClientDev {
    pub(super) fn from_env() -> Result<Option<Self>> {
        std::env::var("AGGR_VITE_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| Self::new(value.trim()))
            .transpose()
    }

    fn new(value: &str) -> Result<Self> {
        let url = Url::parse(value).context("parsing AGGR_VITE_URL")?;
        if url.scheme() != "http"
            || !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            bail!("AGGR_VITE_URL must be a local HTTP origin, such as http://127.0.0.1:5173");
        }
        Ok(Self {
            origin: url.origin().ascii_serialization(),
            script: Regex::new(
                r#"<script src="[^"]*assets/app(?:-[a-f0-9]+)?\.js" defer></script>"#,
            )?,
            stylesheet: Regex::new(
                r#"<link rel="stylesheet" href="[^"]*assets/client(?:-[a-f0-9]+)?\.css"[^>]*>"#,
            )?,
        })
    }

    pub(super) fn inject(&self, body: &[u8]) -> Vec<u8> {
        let html = String::from_utf8_lossy(body);
        if !self.script.is_match(&html) {
            return body.to_vec();
        }
        let scripts = format!(
            "<script type=\"module\" src=\"{0}/@vite/client\"></script><script type=\"module\" src=\"{0}/src/main.ts\"></script>",
            self.origin
        );
        let html = self.script.replace(&html, scripts.as_str());
        self.stylesheet
            .replace_all(&html, "")
            .into_owned()
            .into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_only_compiled_client_assets() {
        let client = ClientDev::new("http://127.0.0.1:5173").unwrap();
        let html = br#"<html><head><link rel="stylesheet" href="assets/style-a1.css"><link rel="stylesheet" href="assets/client-b2.css"></head><body><article>Static article</article><script src="assets/swup-c3.js" defer></script><script src="assets/app-d4.js" defer></script></body></html>"#;
        let output = String::from_utf8(client.inject(html)).unwrap();
        assert!(output.contains("Static article"));
        assert!(output.contains("assets/style-a1.css"));
        assert!(output.contains("assets/swup-c3.js"));
        assert!(output.contains("http://127.0.0.1:5173/@vite/client"));
        assert!(output.contains("http://127.0.0.1:5173/src/main.ts"));
        assert!(!output.contains("assets/app-d4.js"));
        assert!(!output.contains("assets/client-b2.css"));
        assert_eq!(
            client.inject(b"<body>Loading</body>"),
            b"<body>Loading</body>"
        );
    }

    #[test]
    fn accepts_only_local_origins() {
        for url in [
            "http://localhost:5173/",
            "http://127.0.0.1:5173",
            "http://[::1]:5173",
        ] {
            assert!(ClientDev::new(url).is_ok(), "{url}");
        }
        for url in [
            "https://example.com",
            "http://localhost:5173/path",
            "http://user@localhost",
            "http://localhost/?x=1",
            "javascript:alert(1)",
        ] {
            assert!(ClientDev::new(url).is_err(), "{url}");
        }
    }
}
