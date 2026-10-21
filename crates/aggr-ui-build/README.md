# aggr-ui-build

Node-free compilation of aggr's Svelte reader. The crate executes the official Svelte 5.57.2
compiler in an isolated QuickJS runtime, strips module TypeScript with OXC, bundles JavaScript
with Rolldown, and optimizes extracted CSS with Lightning CSS. Runtime, compiler and checker
sources are version-locked under `vendor/packages`; their upstream licenses are retained.

```rust,no_run
# let repository = std::path::Path::new(".");
# let output = std::path::Path::new("target/reader");
let result = aggr_ui_build::build(repository, output)?;
let manifest = std::fs::read(result.directory.join("assets.json"))?;
# Ok::<(), anyhow::Error>(())
```

`repository` contains `web/src`, `web/public`, and the exported page types in
`web/.generated/types`. A build publishes an immutable `output/<hash>` directory and then
atomically replaces `output/current.json` with `{"directory":"<hash>"}`. Consumers retain a
single generation while rendering and serving its assets. Failed or superseded builds leave
the last complete generation available. Generated artifacts and caches are disposable.

The browser entry preserves content-addressed chunks and lazy imports. The preference bootstrap,
service worker and synchronous renderer are self-contained IIFEs. `assets.json` records browser
entry, static module preloads, stylesheet, bootstrap, worker and logical public asset mappings.
Its required `contract` is the SHA256 of the generated page types, so a server can reject a bundle
compiled against a different model before replacing its current generation.
Every generation includes third-party notices. The renderer executes without filesystem,
network or process bindings; no Node executable or npm installation is needed.

CSS optimization uses explicit Chrome 114 capabilities to preserve scroll timelines as separate
longhands. Assuming every proposed shorthand is supported produces invalid animation declarations
in current browsers; the builder regression pins this independently of application layout tests.

`compile_component` and `compile_module` return compiler output and diagnostics for native checks.
`bundle` accepts virtual source replacements and aliases for checker and browser-test tooling.
Its default output is a self-contained ESM entry; set `global_name` for an IIFE, or
`code_splitting` to preserve lazy chunks. These blocking APIs can be called from an existing async
runtime; callers should use a blocking task to keep their request executor available.

Per-source transforms and dependency-aware bundles persist under `web/.generated`. Unchanged
builds verify and reuse their complete generation. Changed builds reuse unaffected targets and
component transforms. The compiler runs on a persistent isolated thread; native bundling uses a
shared bounded worker pool while reserving capacity for background ingestion.

Run `cargo run -p aggr-ui-build --example benchmark` from the source workspace to measure cold
builds, unchanged generations, and twelve alternating CSS/component edits in a temporary copy of
the reader. It reports median and p95 artifact-generation latency without changing source files.
