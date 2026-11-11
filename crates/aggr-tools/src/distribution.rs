use std::collections::BTreeMap;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};

const PACKAGES: &[&str] = &["aggr-ui-model", "aggr-ui-build", "aggr-lib", "aggr"];
const REPOSITORY: &str = "aymericbeaumet/aggr";
const TAP: &str = "aymericbeaumet/homebrew-tap";
const PLATFORMS: &[&str] = &["darwin-arm64", "darwin-amd64", "linux-arm64", "linux-amd64"];

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct Package {
    name: String,
    version: String,
}

fn targets(metadata: &Value, version: &str, reference: Option<&str>) -> Result<Vec<Package>> {
    if let Some(reference) = reference {
        ensure!(
            reference == format!("v{version}"),
            "Tag {reference} does not match v{version}"
        );
    }
    let packages = metadata["packages"]
        .as_array()
        .context("Cargo metadata has no packages")?;
    let selected = PACKAGES
        .iter()
        .map(|name| {
            let package = packages
                .iter()
                .find(|package| package["name"] == *name)
                .with_context(|| format!("Missing workspace package {name}"))?;
            serde_json::from_value::<Package>(package.clone()).context("Reading package version")
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        selected
            .iter()
            .filter(|package| matches!(package.name.as_str(), "aggr" | "aggr-lib"))
            .all(|package| package.version == version),
        "VERSION, aggr and aggr-lib versions must agree"
    );
    Ok(selected)
}

fn registry_contains(package: &Package, body: &str) -> Result<bool> {
    ensure!(
        !body.trim().is_empty(),
        "Empty registry index for {}",
        package.name
    );
    let mut found = false;
    for line in body.lines() {
        let row: Value = serde_json::from_str(line).context("Invalid registry index JSON")?;
        ensure!(
            row["name"] == package.name && row["vers"].is_string() && row["yanked"].is_boolean(),
            "Invalid registry index for {}",
            package.name
        );
        if row["vers"] == package.version {
            ensure!(
                row["yanked"] == false,
                "{} {} is yanked; choose a new version before publishing",
                package.name,
                package.version
            );
            found = true;
        }
    }
    Ok(found)
}

pub(crate) fn without_credentials(command: &mut Command) -> &mut Command {
    command
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_CRATES_IO_TOKEN")
}

fn public_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("aggr-tools/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("Creating public release client")
}

pub async fn publish(args: Vec<String>) -> Result<()> {
    ensure!(
        args.iter().all(|arg| arg == "--allow-dirty"),
        "Publishing accepts only --allow-dirty"
    );
    let output = without_credentials(
        Command::new("cargo")
            .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
            .current_dir(crate::root()),
    )
    .output()
    .context("Reading Cargo metadata")?;
    ensure!(
        output.status.success(),
        "Reading Cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = serde_json::from_slice(&output.stdout).context("Reading Cargo metadata JSON")?;
    let version = std::fs::read_to_string(crate::root().join("VERSION"))?;
    let reference = (std::env::var("GITHUB_REF_TYPE").as_deref() == Ok("tag"))
        .then(|| std::env::var("GITHUB_REF_NAME").context("Reading release tag"))
        .transpose()?;
    let packages = targets(&metadata, version.trim(), reference.as_deref())?;
    let client = public_client()?;
    let mut pending = Vec::new();
    for package in packages {
        let name = &package.name;
        let response = client
            .get(format!(
                "https://index.crates.io/{}/{}/{name}",
                &name[..2],
                &name[2..4]
            ))
            .send()
            .await?;
        let exists = if response.status() == reqwest::StatusCode::NOT_FOUND {
            false
        } else {
            response
                .error_for_status_ref()
                .with_context(|| format!("Checking {name} in crates.io"))?;
            registry_contains(&package, &response.text().await?)?
        };
        if exists {
            println!("{} {} already published; skipping", name, package.version);
        } else {
            pending.push(package);
        }
    }
    if pending.is_empty() {
        println!("Nothing to publish; all crate versions already exist in crates.io.");
        return Ok(());
    }
    ensure!(
        std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
            || std::env::var_os("CARGO_REGISTRY_TOKEN").is_some(),
        "Add the CARGO_REGISTRY_TOKEN repository Actions secret to publish new crate versions"
    );
    crate::package::run(args.clone()).await?;
    let mut command = Command::new("cargo");
    command
        .args(["publish", "--locked", "--registry", "crates-io"])
        .current_dir(crate::root());
    for package in pending {
        command.args(["-p", &format!("{}@{}", package.name, package.version)]);
    }
    crate::command(command.args(args))
}

fn release_version(tag: &str) -> Result<[u64; 3]> {
    let version = tag
        .strip_prefix('v')
        .context("Expected a stable release tag vX.Y.Z")?;
    let fields: Vec<_> = version.split('.').collect();
    ensure!(
        fields.len() == 3
            && fields
                .iter()
                .all(|field| !field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit())),
        "Expected a stable release tag vX.Y.Z"
    );
    Ok([fields[0].parse()?, fields[1].parse()?, fields[2].parse()?])
}

fn checksums(text: &str) -> Result<BTreeMap<String, String>> {
    let mut checksums = BTreeMap::new();
    for line in text.trim().lines() {
        let (digest, filename) = line.split_once(' ').context("Malformed release checksum")?;
        let filename = filename
            .strip_prefix([' ', '*'])
            .context("Malformed release checksum separator")?;
        ensure!(
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                && !filename.is_empty()
                && !filename.contains(['/', '\\'])
                && !filename.chars().any(char::is_whitespace),
            "Malformed release checksum"
        );
        ensure!(
            checksums
                .insert(filename.to_owned(), digest.to_owned())
                .is_none(),
            "Duplicate release checksum"
        );
    }
    for platform in PLATFORMS {
        ensure!(
            checksums.contains_key(&format!("aggr-{platform}.tar.gz")),
            "Missing checksum for aggr-{platform}.tar.gz"
        );
    }
    Ok(checksums)
}

fn formula(tag: &str, text: &str) -> Result<String> {
    release_version(tag)?;
    let sums = checksums(text)?;
    let base = format!("https://github.com/{REPOSITORY}/releases/download/{tag}");
    let blocks: Vec<_> = PLATFORMS
        .iter()
        .map(|platform| {
            let archive = format!("aggr-{platform}.tar.gz");
            format!(
                "      url \"{base}/{archive}\"\n      sha256 \"{}\"",
                sums[&archive]
            )
        })
        .collect();
    Ok(format!(
        r##"class Aggr < Formula
  desc "Git-native feed reader: aggr.toml in, static site out"
  homepage "https://github.com/{REPOSITORY}"
  version "{}"
  license "MIT"
  depends_on "git"

  on_macos do
    on_arm do
{}
    end
    on_intel do
{}
    end
  end

  on_linux do
    on_arm do
{}
    end
    on_intel do
{}
    end
  end

  def install
    bin.install "aggr"
    pkgshare.install "THIRD_PARTY_NOTICES"
  end

  test do
    assert_match "aggr #{{version}}", shell_output("#{{bin}}/aggr --version")
    assert_match "--no-watch", shell_output("#{{bin}}/aggr dev --help")
    system "#{{bin}}/aggr", "init"
    assert_path_exists testpath/"aggr.toml"
  end
end
"##,
        &tag[1..],
        blocks[0],
        blocks[1],
        blocks[2],
        blocks[3]
    ))
}

fn update_formula(
    tag: &str,
    formula: &str,
    previous: Option<&str>,
    sha: Option<&str>,
) -> Result<Option<Value>> {
    let new = release_version(tag)?;
    if let Some(previous) = previous {
        if previous == formula {
            return Ok(None);
        }
        let version = previous
            .lines()
            .find_map(|line| {
                line.strip_prefix("  version \"")
                    .and_then(|line| line.strip_suffix('"'))
            })
            .context("Cannot identify the existing Homebrew version")?;
        let old = release_version(&format!("v{version}"))?;
        ensure!(
            new >= old,
            "Refusing to replace Homebrew {version} with {tag}"
        );
    }
    let mut input = json!({"message": format!("chore: update aggr to {tag}"), "content": base64::engine::general_purpose::STANDARD.encode(formula)});
    if let Some(sha) = sha {
        input["sha"] = sha.into();
    }
    Ok(Some(input))
}

async fn tap_request(
    client: &reqwest::Client,
    token: &str,
    path: &str,
    input: Option<&Value>,
) -> Result<Value> {
    let url = format!("https://api.github.com/repos/{TAP}/contents/{path}");
    let request = match input {
        Some(input) => client.put(url).json(input),
        None => client.get(url),
    };
    let response = request
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .context("Requesting Homebrew tap")?;
    ensure!(
        response.status().is_success(),
        "GitHub tap request failed (HTTP {})",
        response.status()
    );
    response
        .json()
        .await
        .context("Reading Homebrew tap response")
}

pub async fn homebrew(args: Vec<String>) -> Result<()> {
    let tag = args
        .first()
        .context("Usage: homebrew vX.Y.Z [--output PATH]")?;
    release_version(tag)?;
    ensure!(
        args.len() == 1 || (args.len() == 3 && args[1] == "--output"),
        "Usage: homebrew vX.Y.Z [--output PATH]"
    );
    let client = public_client()?;
    let text = client
        .get(format!(
            "https://github.com/{REPOSITORY}/releases/download/{tag}/SHA256SUMS"
        ))
        .send()
        .await?
        .error_for_status()
        .context("Reading release checksums")?
        .text()
        .await?;
    let formula = formula(tag, &text)?;
    if args.len() == 3 {
        std::fs::write(&args[2], formula)?;
        return Ok(());
    }
    let token = std::env::var("GH_TOKEN")
        .context("Set HOMEBREW_TAP_TOKEN as GH_TOKEN for the tap update")?;
    let listing = tap_request(&client, &token, "Formula", None).await?;
    let entry = listing
        .as_array()
        .context("Invalid Homebrew directory listing")?
        .iter()
        .find(|entry| entry["name"] == "aggr.rb");
    let current = if entry.is_some() {
        Some(tap_request(&client, &token, "Formula/aggr.rb", None).await?)
    } else {
        None
    };
    let previous = current
        .as_ref()
        .map(|current| {
            let content = current["content"]
                .as_str()
                .context("Missing Homebrew formula contents")?;
            let content: String = content
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect();
            String::from_utf8(base64::engine::general_purpose::STANDARD.decode(content)?)
                .context("Invalid Homebrew formula encoding")
        })
        .transpose()?;
    let sha = current.as_ref().and_then(|current| current["sha"].as_str());
    if let Some(input) = update_formula(tag, &formula, previous.as_deref(), sha)? {
        tap_request(&client, &token, "Formula/aggr.rb", Some(&input)).await?;
        println!("Updated {TAP} to {tag}");
    } else {
        println!("{tag} is already in the Homebrew tap");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sums() -> String {
        PLATFORMS
            .iter()
            .enumerate()
            .map(|(index, platform)| {
                format!("{}  aggr-{platform}.tar.gz", index.to_string().repeat(64))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn metadata() -> Value {
        json!({"packages": PACKAGES.iter().map(|name| json!({"name": name, "version": if name.starts_with("aggr-ui-") {"0.1.0"} else {"2.0.0"}})).collect::<Vec<_>>()})
    }

    #[test]
    fn publication_orders_prerequisites_and_checks_versions_and_tags() {
        assert_eq!(
            targets(&metadata(), "2.0.0", Some("v2.0.0"))
                .unwrap()
                .iter()
                .map(|package| package.name.as_str())
                .collect::<Vec<_>>(),
            PACKAGES
        );
        assert!(targets(&metadata(), "2.0.1", None).is_err());
        assert!(targets(&metadata(), "2.0.0", Some("v2.0.1")).is_err());
        assert!(targets(&json!({"packages": []}), "2.0.0", None).is_err());
    }

    #[test]
    fn registry_retries_skip_existing_and_reject_yanked_or_malformed_versions() {
        let package = Package {
            name: "aggr-lib".into(),
            version: "2.0.0".into(),
        };
        assert!(
            registry_contains(
                &package,
                r#"{"name":"aggr-lib","vers":"2.0.0","yanked":false}"#
            )
            .unwrap()
        );
        assert!(
            !registry_contains(
                &package,
                r#"{"name":"aggr-lib","vers":"1.0.0","yanked":false}"#
            )
            .unwrap()
        );
        for text in [
            "",
            "invalid",
            r#"{"name":"wrong","vers":"2.0.0","yanked":false}"#,
            r#"{"name":"aggr-lib","vers":"2.0.0","yanked":true}"#,
            r#"{"name":"aggr-lib","vers":null,"yanked":false}"#,
        ] {
            assert!(registry_contains(&package, text).is_err(), "{text}");
        }
    }

    #[test]
    fn preparation_children_cannot_inherit_publication_tokens() {
        let mut command = Command::new("cargo");
        without_credentials(&mut command);
        for name in ["CARGO_REGISTRY_TOKEN", "CARGO_REGISTRIES_CRATES_IO_TOKEN"] {
            assert!(
                command
                    .get_envs()
                    .any(|(key, value)| key == name && value.is_none())
            );
        }
    }

    #[test]
    fn formula_preserves_all_platforms_and_notices() {
        let formula = formula("v2.0.0", &sums()).unwrap();
        for (index, platform) in PLATFORMS.iter().enumerate() {
            assert!(formula.contains(&format!("/v2.0.0/aggr-{platform}.tar.gz")));
            assert!(formula.contains(&format!("sha256 \"{}\"", index.to_string().repeat(64))));
        }
        assert!(formula.contains("depends_on \"git\""));
        assert!(formula.contains("pkgshare.install \"THIRD_PARTY_NOTICES\""));
    }

    #[test]
    fn unsafe_release_tags_and_bad_checksum_sets_fail_closed() {
        for tag in ["main", "v2.0.0-rc.1", "v2.0.0\n", "v2.0.0\""] {
            assert!(formula(tag, &sums()).is_err());
        }
        for text in [
            "".into(),
            sums().lines().skip(1).collect::<Vec<_>>().join("\n"),
            format!("{}\n{}", sums(), sums()),
            "invalid".into(),
            sums().replace("aggr-darwin-arm64", "../aggr-darwin-arm64"),
        ] {
            assert!(formula("v2.0.0", &text).is_err());
        }
    }

    #[test]
    fn tap_updates_preserve_concurrency_and_identical_versions_are_noops() {
        let formula = formula("v2.0.0", &sums()).unwrap();
        assert!(
            update_formula("v2.0.0", &formula, Some(&formula), Some("current-sha"))
                .unwrap()
                .is_none()
        );
        let update = update_formula(
            "v2.0.0",
            &formula,
            Some("  version \"1.12.0\""),
            Some("current-sha"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(update["sha"], "current-sha");
        assert_eq!(
            update["content"],
            base64::engine::general_purpose::STANDARD.encode(&formula)
        );
        assert!(
            update_formula("v2.0.0", &formula, None, None)
                .unwrap()
                .unwrap()
                .get("sha")
                .is_none()
        );
    }

    #[test]
    fn older_retries_and_unknown_formulas_cannot_overwrite_the_tap() {
        for previous in ["  version \"2.1.0\"", "  version \"10.0.0\"", "unknown"] {
            assert!(update_formula("v2.0.0", "new", Some(previous), Some("sha")).is_err());
        }
    }
}
