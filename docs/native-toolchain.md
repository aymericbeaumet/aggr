# Native development toolchain

The project builds, develops, checks and packages with Rust and pinned native tools. No JavaScript
host or frontend package manager is required. Svelte remains the reader framework: the official
compiler runs inside QuickJS, and native Rolldown assembles the browser and server bundles.

## Reproduce the development environment

Install [mise](https://mise.jdx.dev/installing-mise.html) first; CI uses **2026.11.6** and the
configuration enforces a minimum supported version. From a fresh checkout:

```sh
mise trust
mise run setup
mise run check
```

Mise is the only development tool installer. Setup installs the exact versions in `mise.toml`
through `mise.lock`, then fetches the Rust dependencies from `Cargo.lock`. Installations use
`--locked`, so a missing platform entry fails instead of silently choosing another download.
The frontend sources and their checksums are committed; setup never runs a JavaScript package
manager. Use `mise run` tasks or `mise exec -- cargo …` to select the project tools without
depending on a shell's existing Rust installation.

The host still supplies its SDK, C/C++ compiler and native libraries: Xcode Command Line Tools
on macOS, Visual Studio's C++ build tools and LLVM on Windows, and a C/C++ build environment plus
libclang on Linux. See [native build requirements](interoperability.md#http-extraction-and-native-builds).
Mise supplies CMake, Ninja and, on Windows x64, NASM. This pins project tools and dependency
inputs; it does not promise identical binary bytes across different host SDKs or operating systems.
Chrome for Testing and ChromeDriver are pinned together on Linux x64, macOS and Windows x64;
Google does not publish the corresponding Linux or Windows ARM64 browser packages.

When updating a tool, change its exact version in `mise.toml` and refresh every supported
platform before committing both files:

```sh
mise lock --platform linux-x64,linux-arm64,macos-arm64,macos-x64,windows-x64,windows-arm64
mise run setup
mise run check
```

HTTP tools also require the new archive's SHA-256 in `mise.toml`. CI pins third-party actions
to full commit hashes, and Dependabot proposes updates to those pins.
`mise run workflow-check` uses pinned jactionlint with its `correctness` profile, the
[documented migration path](https://jactionlint.jdx.dev/actionlint) from actionlint.
The MSRV check is the deliberate second Rust toolchain: `mise run msrv` selects the exact
`package.rust-version` from `Cargo.toml` through mise, while its task runner uses the normal
project toolchain. CI installs both explicitly; the Rust installer verifies release manifests.

## Everyday commands

```sh
mise run setup
mise run dev
mise run check
mise run release
mise run verify-release
mise run package
```

`cargo run -- dev` starts the same source workflow. Set `AGGR_CONFIG` to preview a specific
configuration; `mise run dev -- --port 3000` chooses the public port. `aggr dev --no-watch`
previews the embedded reader. Installed binaries serve the embedded UI without compiler tools.
Cargo's optimization profile never selects a different reader implementation or feature set.

## Build and edit loop

`aggr-ui-build` is the shared compiler library. It resolves the checked-in frontend dependencies,
compiles Svelte components and rune modules, transforms TypeScript, assembles the executable module
graph and extracts CSS. Browser, bootstrap, worker and isolated server-renderer artifacts form one
complete UI bundle. The manifest resolves immutable asset URLs and eager dependencies.

The official compiler and runtime dependencies are vendored with their licenses and checksums.
Building the reader does not install packages or fetch remote assets. `aggr-tools` owns source
watching, verification, browser tests and release preparation; mise invokes those Rust commands.
`mise run vendor -- verify` checks the frozen source inventory. To update a package, edit its pinned
version in `crates/aggr-ui-build/vendor/packages.json`, then run `mise run vendor -- update` and
review the source and license diff. The native updater verifies upstream SHA-512 before extraction
and records local SHA-256 hashes; it executes no package scripts. `vendor -- record` acknowledges
reviewed local changes without downloading anything.

Pins follow upstream compatibility rather than forcing every package onto the same major version.
The vendored TypeScript 6 JavaScript compiler API supports `svelte2tsx` projections inside QuickJS;
the separate TypeScript 7 native executable checks those projections. Keep both until `svelte2tsx`
supports a replacement compiler API, and verify projection diagnostics when updating either.

During iteration, the compiler retains transformation caches. A complete UI generation becomes
visible atomically; malformed or superseded builds never replace the last working generation.
Rust pins each request to a single archive/UI pair and evaluates the same renderer used for
publication. QuickJS instances are associated with renderer code, rather than stylesheet changes.

A Svelte, TypeScript or CSS save rebuilds the affected UI outputs and reloads the page. It does
not invoke Cargo, restart Rust, reprocess archived articles, or rebuild unchanged search data.
Rust source changes compile a replacement worker while the current reader remains available.
The supervisor preserves the public origin and restores the last successful worker if startup fails.
After changing the native compiler, supervisor, or vendored compiler dependencies themselves,
restart the contributor command to rebuild its tooling. Ordinary reader edits never require this.

## Preview and publication

Preview renders requested pages and prepares media/search on demand. `build` materializes and
validates the complete publication, including its output budget. Both use the same components,
page models, asset policies and rendering implementation. Canonical metadata follows configuration;
the preview HTTP server prevents crawling with `X-Robots-Tag: noindex`.

PWA behavior follows configuration in both modes. Reload is announced through the existing updates
request and an uncached event stream, keeping page bytes identical to their offline revisions.
Preview never clears saved downloads merely to expose a template change. Offline catalogue work
is single-flight per snapshot and can outlive a disconnected request; downloads become ready only
after every required resource passes integrity verification.

## Verification and distribution

Svelte diagnostics and virtual TypeScript projections run inside QuickJS. The pinned native
TypeScript executable checks templates and regular/worker code; it is launched directly, without
a JavaScript wrapper. Missing checkers are errors, not permission to skip diagnostics.

Reader unit contracts execute in isolated pages in the pinned browser, driven by Rust. Full browser
contracts additionally exercise real generated archives, navigation and offline behavior. Rendering
tests use the embedded QuickJS renderer. Release preparation, formula generation and registry
publication are Rust commands, with publication credentials excluded from build subprocesses.

Publish `aggr-ui-model` and `aggr-ui-build` before `aggr-lib` and `aggr`. The reusable release command
skips already published versions so retries do not republish completed prerequisites. Compiler
library changes require an appropriate `aggr-ui-build` version bump before a later publication.
