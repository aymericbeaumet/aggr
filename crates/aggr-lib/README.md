# aggr-lib

Git-native feed ingestion, append-only archival storage, and static reader generation.
The `aggr` command-line application uses this library for its workflows.

```rust
use aggr_lib::config::Config;

let config = Config::parse(r#"
[[sources]]
url = "https://blog.rust-lang.org/feed.xml"
"#)?;
assert_eq!(config.sources()?.len(), 1);
# Ok::<(), anyhow::Error>(())
```

The `config`, `sources`, `store`, and `site` modules expose the underlying building
blocks; `commands` exposes complete workflows. Network access belongs to ingestion.
Rendering consumes local archived content and embedded reader assets.

Published crates include the compiled reader and build with Cargo alone.
Native dependencies for the HTTP challenge fallback are described in
[interoperability](https://github.com/aymericbeaumet/aggr/blob/main/docs/interoperability.md).
See the [repository README](https://github.com/aymericbeaumet/aggr) for CLI installation and use,
and [library development](https://github.com/aymericbeaumet/aggr/blob/main/docs/library.md)
for packaging and publication.
