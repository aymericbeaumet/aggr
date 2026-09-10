//! Instagram profiles: consume exposed post cards and explain access-limited profile shells.

use anyhow::{Context as _, Result, bail};
use scraper::{Html, Selector};
use url::Url;

use super::{Context, Fetch, SourceMeta, Validators};
use crate::{
    config::Source,
    http::Response,
    model::{RawItem, sha1_hex},
};

pub fn is_profile_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https") && instagram_host(url) && username(url).is_some()
}

fn instagram_host(url: &Url) -> bool {
    matches!(
        url.host_str(),
        Some("instagram.com" | "www.instagram.com" | "m.instagram.com")
    )
}

fn username(url: &Url) -> Option<&str> {
    let name = url.path().trim_matches('/');
    (!name.is_empty()
        && name.len() <= 30
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
        && !matches!(
            name,
            "accounts"
                | "explore"
                | "direct"
                | "reel"
                | "reels"
                | "p"
                | "stories"
                | "about"
                | "developer"
                | "legal"
                | "challenge"
                | "checkpoint"
        ))
    .then_some(name)
}

pub async fn fetch(url: &Url, source: &Source, ctx: &Context<'_>) -> Result<Fetch> {
    let previous = if ctx.state.identity == source.identity {
        Validators::from_state(ctx.state)
    } else {
        Validators::default()
    };
    let response = super::feed::request(url, source, ctx, &previous)
        .await
        .context(
            "loading the Instagram profile; Instagram may require login or restrict this request",
        )?;
    let Response::Ok(body) = response else {
        return Ok(Fetch::Unchanged {
            validators: previous,
        });
    };
    let (meta, items) = parse(&body.html_text(), url, &body.final_url)?;
    let validators = Validators {
        etag: body.etag,
        last_modified: body.last_modified,
        body_hash: Some(sha1_hex(&body.bytes)),
        resolved_url: Some(url.to_string()),
    };
    if validators.body_hash == previous.body_hash {
        return Ok(Fetch::Unchanged { validators });
    }
    Ok(Fetch::Changed {
        validators,
        meta,
        items,
    })
}

fn parse(page: &str, profile: &Url, final_url: &Url) -> Result<(SourceMeta, Vec<RawItem>)> {
    let name = username(profile).context("expected an Instagram profile URL")?;
    let login_response = serde_json::from_str::<serde_json::Value>(page)
        .is_ok_and(|value| value["require_login"] == true || value["login_required"] == true);
    if final_url.path().starts_with("/accounts/login") || login_response {
        bail!(
            "Instagram requires login to read @{name}; aggr does not log in. Use the creator's website or feed instead"
        );
    }
    if final_url.path().starts_with("/challenge") || final_url.path().starts_with("/checkpoint") {
        bail!(
            "Instagram returned a verification challenge for @{name}; aggr cannot complete it. Use the creator's website or feed instead"
        );
    }
    if profile.origin() != final_url.origin()
        && !(instagram_host(profile) && instagram_host(final_url))
    {
        bail!("Instagram redirected @{name} outside its provider; no posts were imported");
    }
    if let Ok((meta, mut items)) = super::html::extract(page, final_url) {
        items.retain(|item| {
            Url::parse(&item.link).is_ok_and(|url| {
                let same_provider = url.origin() == profile.origin()
                    || (instagram_host(&url) && instagram_host(profile));
                let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
                let post = match parts.as_slice() {
                    [kind, code] => Some((*kind, *code)),
                    [owner, kind, code] if owner.eq_ignore_ascii_case(name) => Some((*kind, *code)),
                    _ => None,
                };
                same_provider
                    && post.is_some_and(|(kind, code)| {
                        matches!(kind, "p" | "reel" | "tv")
                            && !code.is_empty()
                            && code.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
                            })
                    })
            })
        });
        if !items.is_empty() {
            return Ok((meta, items));
        }
    }
    let document = Html::parse_document(page);
    let selector = Selector::parse("h1, h2, meta[property='og:description']")
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    if document.select(&selector).any(|element| {
        let text = element
            .value()
            .attr("content")
            .map(str::to_owned)
            .unwrap_or_else(|| element.text().collect::<String>());
        text.trim().eq_ignore_ascii_case("This Account is Private")
    }) {
        bail!(
            "Instagram identifies @{name} as a private account; its posts cannot be imported publicly. Use a source its owner makes public instead"
        );
    }
    bail!(
        "Instagram returned a profile shell for @{name} without public posts; this profile cannot currently be imported unauthenticated. Use the creator's website or feed instead; aggr does not log in to Instagram"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> Url {
        Url::parse("https://www.instagram.com/finntonry/").unwrap()
    }

    #[test]
    fn distinguishes_profiles_from_posts_and_unrelated_hosts() {
        assert!(is_profile_url(&profile()));
        assert!(is_profile_url(
            &Url::parse("https://instagram.com/a.name_123?igsh=share").unwrap()
        ));
        for value in [
            "https://notinstagram.com/finntonry",
            "https://www.instagram.com/accounts/login",
            "https://www.instagram.com/p/POST123",
            "https://www.instagram.com/explore",
            "https://www.instagram.com/",
        ] {
            assert!(!is_profile_url(&Url::parse(value).unwrap()), "{value}");
        }
    }

    #[test]
    fn metadata_only_profiles_explain_that_public_posts_are_unavailable() {
        let page = r#"<html><head><meta property="og:title" content="FINN TONRY (@finntonry) • Instagram photos and videos"><meta property="og:description" content="1M Followers, 172 Posts"></head><body><a href="/accounts/login/">Log in</a><script type="application/json">{"number_of_preloaded_posts_in_timeline_for_crawler":0}</script></body></html>"#;
        let error = parse(page, &profile(), &profile()).unwrap_err().to_string();
        assert!(error.contains("without public posts"), "{error}");
        assert!(error.contains("creator's website or feed"), "{error}");
        assert!(
            !error.contains("requires login"),
            "a login link alone does not prove login is required"
        );
    }

    #[test]
    fn login_challenge_and_private_pages_have_specific_failures() {
        for (page, path, expected) in [
            (
                "<title>Instagram</title>",
                "/accounts/login/",
                "requires login",
            ),
            (
                "<title>Instagram</title>",
                "/challenge/",
                "verification challenge",
            ),
            (
                "<h2>This Account is Private</h2>",
                "/finntonry/",
                "private account",
            ),
            (
                r#"{"require_login":true,"status":"fail"}"#,
                "/finntonry/",
                "requires login",
            ),
        ] {
            let error = parse(page, &profile(), &profile().join(path).unwrap())
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn imports_only_exposed_instagram_posts_without_profile_navigation() {
        let page = r#"<title>Finn Tonry</title><article><h2><a href="/p/POST123/">A public post</a></h2><time datetime="2026-09-08">Today</time><p>Public caption.</p></article><article><h2><a href="https://unrelated.example/p/FAKE/">An unrelated story</a></h2></article><h2><a href="/accounts/login/">Log in</a></h2>"#;
        let (meta, items) = parse(page, &profile(), &profile()).unwrap();
        assert_eq!(meta.title.as_deref(), Some("Finn Tonry"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].link, "https://www.instagram.com/p/POST123/");
        assert_eq!(items[0].title, "A public post");
    }

    #[tokio::test]
    async fn profile_requests_keep_headers_and_fail_without_probing_other_endpoints() {
        use httpmock::prelude::*;
        crate::http::install_crypto_provider();
        let server = MockServer::start();
        let mut profile_page = server.mock(|when, then| {
            when.method(GET)
                .path("/finntonry/")
                .header("x-reader-test", "configured");
            then.status(200).body(
                "<title>Instagram</title><meta property='og:description' content='172 Posts'>",
            );
        });
        let config = crate::config::Config::parse(&format!(
            "[fetch]\nretries=0\n[[sources]]\nurl={:?}\nheaders={{'X-Reader-Test'='configured'}}",
            server.url("/finntonry/")
        ))
        .unwrap();
        let source = config.sources().unwrap().remove(0);
        let url = Url::parse(&server.url("/finntonry/")).unwrap();
        let client = crate::http::Client::new(&config.fetch).unwrap();
        let mut state = crate::store::SourceState::default();
        let cache = tempfile::tempdir().unwrap();
        let context = Context {
            client: &client,
            state: &state,
            cache_dir: cache.path(),
        };
        let error = match fetch(&url, &source, &context).await {
            Err(error) => error,
            Ok(_) => panic!("a metadata-only profile must fail"),
        };
        assert!(error.to_string().contains("without public posts"));
        profile_page.assert_calls(1);
        profile_page.delete();

        let mut denied = server.mock(|when, then| {
            when.method(GET)
                .path("/finntonry/")
                .header("x-reader-test", "configured");
            then.status(401)
                .json_body(serde_json::json!({"require_login":true,"status":"fail"}));
        });
        let error = match fetch(&url, &source, &context).await {
            Err(error) => error,
            Ok(_) => panic!("access denial must fail"),
        };
        assert!(format!("{error:#}").contains("Instagram"));
        assert!(format!("{error:#}").contains("401"));
        denied.assert_calls(1);
        denied.delete();

        let mut public = server.mock(|when, then| {
            when.method(GET).path("/finntonry/").header("x-reader-test", "configured");
            then.status(200).header("etag", "post-version").body("<title>Finn Tonry</title><article><h2><a href='/p/POST123/'>Public post</a></h2></article>");
        });
        let Fetch::Changed {
            validators, items, ..
        } = fetch(&url, &source, &context).await.unwrap()
        else {
            panic!("public post data must be imported");
        };
        assert_eq!(items.len(), 1);
        public.assert_calls(1);
        public.delete();
        validators.apply(&mut state);
        state.identity = source.identity.clone();
        let unchanged = server.mock(|when, then| {
            when.method(GET)
                .path("/finntonry/")
                .header("x-reader-test", "configured")
                .header("if-none-match", "post-version");
            then.status(304);
        });
        let context = Context {
            client: &client,
            state: &state,
            cache_dir: cache.path(),
        };
        assert!(matches!(
            fetch(&url, &source, &context).await.unwrap(),
            Fetch::Unchanged { .. }
        ));
        unchanged.assert_calls(1);
    }
}
