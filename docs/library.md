# Library and CLI

`aggr-lib` contains configuration, ingestion, append-only Git storage, rendering, and command
workflows. Its Rust import name is `aggr_lib`. The root `aggr` package provides argument parsing,
logging, the Tokio runtime, and dispatch through the library. `aggr-ui-model` holds the shared
Rust/TypeScript page contract. See the [library example](../crates/aggr-lib/README.md).

Workspace builds compile the reader from `web/` with the pinned Node toolchain. Published library
archives contain the precompiled reader and embedded defaults, schema, and starter configuration;
consumers need Rust and the [native build prerequisites](interoperability.md), but no Node.
The published `aggr` CLI crate uses that same library archive. Once the CLI crate is published,
install it with `cargo install --locked aggr`; this compiles Rust and native dependencies without
Node. The optional `dev-worker` feature builds the contributor supervisor's extra binary; normal
Cargo installs build only `aggr`.
Installed binaries run `aggr dev` with the native preview server. In a source checkout, use
`aggr dev --native` to select that same mode or ordinary `aggr dev` for Vite and Rust restarts.

## Packaging and publishing

Keep `VERSION`, the root CLI version, and `crates/aggr-lib/Cargo.toml` aligned. The model crate has
its own version; both library dependencies on it must name a published version.

Run `mise run package` from a clean checkout to prepare the reader and resources and verify all three
crate archives, including compiling the extracted packages. The generated publication inputs live
under the ignored `crates/aggr-lib/.generated/publish/`; they are not source assets.

Publishing runs in [the publish crates workflow](../.github/workflows/publish.yml). Add a repository
Actions secret named **`CARGO_REGISTRY_TOKEN`**, containing a crates.io token allowed to publish
`aggr-ui-model`, `aggr-lib`, and `aggr`. No local `cargo login` is needed.

For the initial publication, open **Actions → publish crates → Run workflow** and select the
branch containing the library packages and this workflow. It publishes the versions checked into
that ref: currently `aggr-ui-model` **0.1.0**, then `aggr-lib` **2.0.0**, then `aggr` **2.0.0**. The workflow must first
exist on the repository's default branch for [manual runs](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow)
to be available. Existing binary
release tags do not need to be recreated. Future `vX.Y.Z` tags call the same publishing workflow
after the binary release and its CI checks succeed; a tag must match the library version.

The workflow runs `mise run publish`, which checks Cargo metadata, aligned versions, and the public
registry index before doing any package work. Already published versions are explicitly skipped;
if all three exist, rerunning is a successful no-op, even without a token. Yanked versions and failed
registry lookups stop the run. When new versions remain, `mise run package` prepares and verifies the
self-contained archives, then Cargo publishes only those missing versions in dependency order
and [waits for indexing](https://doc.rust-lang.org/cargo/commands/cargo-publish.html). The initial
`mise run package` preparation runs without the registry token; Cargo receives it for publishing.
The workflow does not upload separate reader artifacts. The CLI package includes its entry
sources, lockfile, README, license, and third-party notices; development scripts, fixtures, and
the frontend source tree stay out of that archive.

If publishing fails after uploading the model, wait until its version appears in the registry
index, then rerun the workflow on the same ref; it will skip published prerequisites and continue
with the library or CLI. Crate versions are immutable: changes to a published
model, library, or CLI require a version bump. `mise run publish` uses the same helper for an explicitly
requested maintainer publication, without pushing Git commits. For an explicitly authorized dirty
checkout, pass `--allow-dirty`: `mise run package -- --allow-dirty` or
`mise run publish -- --allow-dirty`.

Run library unit tests with `cargo test -p aggr-lib <filter>`, CLI integration tests with
`cargo test -p aggr --test cli`, and all workspace checks with `mise run check`.
