//! Repository URL recognition without network probes or transport changes for cloning.

use anyhow::{Context, Result, bail};
use url::Url;

pub(super) fn inferred(value: &str) -> bool {
    let Ok(url) = parse(value) else { return false };
    matches!(url.scheme(), "ssh" | "git")
        || url.path().trim_end_matches('/').ends_with(".git")
        || (matches!(
            url.host_str(),
            Some("github.com" | "gitlab.com" | "bitbucket.org" | "codeberg.org")
        ) && url
            .path_segments()
            .is_some_and(|segments| segments.filter(|part| !part.is_empty()).count() == 2))
}

pub(super) fn parse(value: &str) -> Result<Url> {
    if value.chars().any(char::is_whitespace) {
        bail!("repository URLs cannot contain whitespace");
    }
    let normalized = if !value.contains("://") {
        let (host, path) = value
            .split_once(':')
            .context("repository URL must use HTTP(S), SSH, or git")?;
        if !host.contains('@') || host.contains('/') || path.is_empty() {
            bail!("invalid SSH repository URL");
        }
        format!("ssh://{host}/{}", path.trim_start_matches('/'))
    } else {
        value.to_owned()
    };
    let mut url = Url::parse(&normalized).context("invalid repository URL")?;
    if !matches!(url.scheme(), "https" | "http" | "ssh" | "git") || url.host_str().is_none() {
        bail!("repository URL must use HTTP(S), SSH, or git");
    }
    if url.fragment().is_some() {
        bail!("repository URLs cannot have fragments; set `branch` separately");
    }
    let path = url.path().trim_end_matches('/').to_owned();
    if path.is_empty() {
        bail!("repository URL must include a repository path");
    }
    url.set_path(&path);
    Ok(url)
}

pub(super) fn identity(value: &str) -> Option<String> {
    let mut url = parse(value).ok()?;
    let path = url.path().trim_end_matches(".git").to_owned();
    url.set_path(&path);
    // GitHub's SSH username is fixed; it identifies the transport, not the repository.
    if url.host_str() == Some("github.com") && url.port().is_none() {
        if url.scheme() == "ssh" && url.username() == "git" && url.password().is_none() {
            url = Url::parse(&format!("https://github.com{}", url.path())).ok()?;
        } else if matches!(url.scheme(), "http" | "https" | "git") {
            let _ = url.set_scheme("https");
        }
    }
    Some(url.to_string())
}

pub(crate) fn public(url: &Url, strip_query: bool) -> Option<String> {
    let browser = if matches!(url.scheme(), "http" | "https") {
        url.clone()
    } else {
        Url::parse(&format!("https://{}{}", url.host_str()?, url.path())).ok()?
    };
    let mut browser = browser;
    let path = browser.path().trim_end_matches(".git").to_owned();
    browser.set_path(&path);
    Some(super::public_url(&browser, strip_query))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_transports_are_validated_without_changing_ssh_ports() {
        let url = parse("git@git.example.org:team/reader.git").unwrap();
        assert_eq!(url.as_str(), "ssh://git@git.example.org/team/reader.git");
        let url = parse("ssh://git@git.example.org:2222/team/reader.git").unwrap();
        assert_eq!(url.port(), Some(2222));
        for invalid in [
            "file:///tmp/repo.git",
            "ext::some-command",
            "https://example.org",
            "https://example.org/repo.git#branch",
            "git@example.org:",
            "ssh://git@example.org/repo with spaces",
        ] {
            assert!(parse(invalid).is_err(), "{invalid}");
        }
    }
}
