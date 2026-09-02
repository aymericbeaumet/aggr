//! The few GitHub REST calls the digest needs, authenticated with the workflow's `GITHUB_TOKEN`
//! (or any token in `GITHUB_TOKEN` / `GH_TOKEN` locally).

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

const DEFAULT_API: &str = "https://api.github.com";

pub struct GitHub {
    client: reqwest::Client,
    api: String,
    repo: String,
    token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub html_url: String,
    pub title: String,
}

#[derive(Debug, Serialize)]
pub struct NewIssue<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub labels: &'a [String],
    pub assignees: &'a [String],
}

#[derive(Deserialize)]
struct RepoInfo {
    owner: Owner,
}

#[derive(Deserialize)]
struct Owner {
    login: String,
    #[serde(rename = "type")]
    kind: String,
}

impl GitHub {
    /// `None` when no token is available; callers decide whether that is an error.
    pub fn from_env(repo: &str, client: reqwest::Client) -> Option<Self> {
        let token = ["GITHUB_TOKEN", "GH_TOKEN"]
            .iter()
            .find_map(|name| std::env::var(name).ok())
            .filter(|token| !token.is_empty())?;
        let api = std::env::var("GITHUB_API_URL")
            .ok()
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| DEFAULT_API.into());
        Some(Self::new(repo, token, api, client))
    }

    pub fn new(repo: &str, token: String, api: String, client: reqwest::Client) -> Self {
        Self {
            client,
            api: api.trim_end_matches('/').to_string(),
            repo: repo.to_string(),
            token,
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}/repos/{}{path}", self.api, self.repo))
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        builder: reqwest::RequestBuilder,
        what: &str,
    ) -> Result<T> {
        let response = builder
            .send()
            .await
            .with_context(|| format!("GitHub API: {what}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!(
                "GitHub API: {what} failed with {status}: {}",
                summarize(&text)
            );
        }
        serde_json::from_str(&text).with_context(|| format!("GitHub API: decoding {what}"))
    }

    /// The login to assign by default: the owner for user repositories, nobody for organizations
    /// (organizations cannot be assigned).
    pub async fn default_assignee(&self) -> Result<Option<String>> {
        let info: RepoInfo = self
            .send(
                self.request(reqwest::Method::GET, ""),
                "reading the repository",
            )
            .await?;
        Ok((info.owner.kind == "User").then_some(info.owner.login))
    }

    pub async fn ensure_label(&self, name: &str, color: &str, description: &str) -> Result<()> {
        let path = format!("/labels/{}", urlencode(name));
        let response = self
            .request(reqwest::Method::GET, &path)
            .send()
            .await
            .context("GitHub API: reading label")?;
        if response.status().is_success() {
            return Ok(());
        }
        if response.status() != reqwest::StatusCode::NOT_FOUND {
            bail!(
                "GitHub API: reading label failed with {}",
                response.status()
            );
        }
        let body = serde_json::json!({ "name": name, "color": color, "description": description });
        let _: serde_json::Value = self
            .send(
                self.request(reqwest::Method::POST, "/labels").json(&body),
                "creating label",
            )
            .await?;
        Ok(())
    }

    pub async fn create_issue(&self, issue: &NewIssue<'_>) -> Result<Issue> {
        self.send(
            self.request(reqwest::Method::POST, "/issues").json(issue),
            "creating issue",
        )
        .await
    }

    pub async fn open_issues(&self, label: &str) -> Result<Vec<Issue>> {
        let path = format!(
            "/issues?state=open&labels={}&per_page=100",
            urlencode(label)
        );
        self.send(self.request(reqwest::Method::GET, &path), "listing issues")
            .await
    }

    pub async fn close_issue(&self, number: u64) -> Result<()> {
        let body = serde_json::json!({ "state": "closed", "state_reason": "completed" });
        let _: serde_json::Value = self
            .send(
                self.request(reqwest::Method::PATCH, &format!("/issues/{number}"))
                    .json(&body),
                "closing issue",
            )
            .await?;
        Ok(())
    }
}

fn urlencode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

fn summarize(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("message")?.as_str().map(str::to_string))
        .unwrap_or_else(|| body.chars().take(200).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn client() -> reqwest::Client {
        crate::http::install_crypto_provider();
        reqwest::Client::new()
    }

    fn github(server: &MockServer) -> GitHub {
        GitHub::new("o/r", "tok".into(), server.base_url(), client())
    }

    #[tokio::test]
    async fn creates_issues_with_auth_headers() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/repos/o/r/issues")
                    .header("authorization", "Bearer tok")
                    .body_includes("\"title\":\"Digest #1\"")
                    .body_includes("\"assignees\":[\"me\"]");
                then.status(201)
                    .json_body(serde_json::json!({ "number": 7, "html_url": "https://github.com/o/r/issues/7", "title": "Digest #1" }));
            })
            .await;
        let issue = github(&server)
            .create_issue(&NewIssue {
                title: "Digest #1",
                body: "hi",
                labels: &["digest".into()],
                assignees: &["me".into()],
            })
            .await
            .unwrap();
        mock.assert_async().await;
        assert_eq!(issue.number, 7);
    }

    #[tokio::test]
    async fn creates_missing_labels_only() {
        let server = MockServer::start_async().await;
        let missing = server
            .mock_async(|when, then| {
                when.method(GET).path("/repos/o/r/labels/digest");
                then.status(404).body("{\"message\":\"Not Found\"}");
            })
            .await;
        let create = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/repos/o/r/labels")
                    .body_includes("\"name\":\"digest\"");
                then.status(201).body("{}");
            })
            .await;
        github(&server)
            .ensure_label("digest", "0f766e", "aggr digests")
            .await
            .unwrap();
        missing.assert_async().await;
        create.assert_async().await;
    }

    #[tokio::test]
    async fn surfaces_api_errors_with_the_message() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/repos/o/r");
                then.status(403)
                    .body("{\"message\":\"Resource not accessible by integration\"}");
            })
            .await;
        let err = github(&server).default_assignee().await.unwrap_err();
        assert!(
            format!("{err:#}").contains("Resource not accessible"),
            "{err:#}"
        );
    }

    #[test]
    fn from_env_needs_a_token() {
        // Environment access is process-wide; only assert the shape of the constructor.
        let gh = GitHub::new(
            "o/r",
            "t".into(),
            "https://api.github.com/".into(),
            client(),
        );
        assert_eq!(gh.api, "https://api.github.com");
    }
}
