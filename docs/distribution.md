# Releases and installation

The published `aggr` v2.0.0 CLI installs through mise or GitHub release downloads. This checkout
adds the Homebrew and crates.io publication pipelines; neither channel has been published yet.
`aggr-lib` exposes the engine to Rust applications; `aggr-ui-model` is the shared reader contract
and `aggr-ui-build` compiles its UI. The [crates workflow](../.github/workflows/publish.yml)
publishes all four crates; see [library packaging](library.md).

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
are skipped. **Actions → publish Homebrew → Run workflow** accepts a stable release tag produced
by the native release pipeline above and uses its `SHA256SUMS`. Its archives must contain
`THIRD_PARTY_NOTICES`, and its CLI must support `dev --no-watch`; the workflow installs and tests
that binary before publishing the formula. The existing v2.0.0 archives predate both requirements
and cannot be used for this Homebrew rollout. Keep that release immutable and publish a new
version before enabling this channel. An identical formula update is a no-op, an older
version cannot replace a newer one, and concurrent tap changes cause the update to fail safely.
These workflows can be retried independently after correcting a token or publication failure.

For an initial rollout, the new GitHub release must exist before the Homebrew workflow runs.
After the tap update succeeds, users can run `brew install aymericbeaumet/tap/aggr`. Cargo
installation (`cargo install --locked aggr`) becomes available after all four crates have been
published. Update the installation documentation when each channel becomes available.
Current installation commands and platform prerequisites are in
[hosting](hosting.md#installing-the-binary).

## Local verification

`mise run package` prepares the compiled reader and verifies all four self-contained Cargo packages.
Published packages embed those assets. Rust tooling tests cover
publication order, retries, checksum validation, and tap updates without making
external writes. The Homebrew workflow installs and tests the actual release binary before
updating the shared tap.
