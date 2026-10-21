# Releases and installation

The `aggr` CLI installs through Homebrew, mise, or Cargo. `aggr-lib` exposes its engine to Rust
applications; `aggr-ui-model` is the shared reader contract and `aggr-ui-build` compiles its UI. All four crates are published through
the [crates workflow](../.github/workflows/publish.yml); see [library packaging](library.md).

## Release pipeline

Keep `VERSION`, the root package, and `aggr-lib` versions aligned. Commit the release and tag it
`vX.Y.Z`. The [release workflow](../.github/workflows/release.yml) verifies versions, runs CI, and
builds six archives: Linux, macOS, and Windows on amd64 and arm64. GitHub Releases receives the
archives, licenses, third-party notices, and `SHA256SUMS`. The release then starts two independent
publication jobs:

- crates.io publishes missing `aggr-ui-model`, `aggr-ui-build`, `aggr-lib`, and `aggr` versions in dependency order.
- Homebrew validates a formula against the published archive on a macOS runner, then updates
  `Formula/aggr.rb` in `aymericbeaumet/homebrew-tap`. The formula uses prebuilt macOS and Linux
  archives for both architectures, verifies their release checksums, and depends on Git.

mise uses the same GitHub release archives through `github:aymericbeaumet/aggr`; it needs no separate
registry publication or token. Existing stable reusable workflow tags keep their major-channel
contract; a binary release does not move those tags.

## Repository Actions secrets

| Secret | Required access |
|---|---|
| `CARGO_REGISTRY_TOKEN` | crates.io permission to publish `aggr-ui-model`, `aggr-ui-build`, `aggr-lib`, and `aggr`; include permission for new crate names during the initial publication. |
| `HOMEBREW_TAP_TOKEN` | Contents read/write on `aymericbeaumet/homebrew-tap`; a fine-grained token must select that repository. |

GitHub release creation uses the workflow's own `GITHUB_TOKEN`. The tap token is passed only to
the tap-update step; public checksum downloads and formula testing do not receive it. Cargo package
preparation runs without the registry token.

## Retry or publish an existing release

Once the workflow files are available on the default branch, **Actions → publish crates → Run
workflow** can publish the versions in the selected branch or tag. Already published versions
are skipped. **Actions → publish Homebrew → Run workflow** accepts an existing stable release
tag, such as `v2.0.0`, and uses its `SHA256SUMS`. An identical formula update is a no-op, an older
version cannot replace a newer one, and concurrent tap changes cause the update to fail safely.
These workflows can be retried independently after correcting a token or publication failure.

For an initial rollout, the GitHub release must exist before the Homebrew workflow runs. Cargo
installation becomes available after all four crates have been published. Installation commands
and platform prerequisites are in [hosting](hosting.md#installing-the-binary).

## Local verification

`mise run package` prepares the compiled reader and verifies all four self-contained Cargo packages.
Published packages embed those assets. Rust tooling tests cover
publication order, retries, checksum validation, and tap updates without making
external writes. The Homebrew workflow installs and tests the actual release binary before
updating the shared tap.
