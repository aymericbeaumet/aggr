# aggr [![ci](https://github.com/aymericbeaumet/aggr/actions/workflows/ci.yml/badge.svg)](https://github.com/aymericbeaumet/aggr/actions/workflows/ci.yml)

[**Read the live demo**](https://aggr.aymericbeaumet.com) · [Try locally](#try-locally) · [Create your reader](#create-your-reader)

**A feed reader with a Markdown archive in Git.**

Give aggr a website URL. It finds the feed, saves each new item as Markdown in your Git
repository, and publishes a static site you can search and read on your phone or computer.
Keep the captured text after the original page disappears, inspect its history with Git, or
take the Markdown into another tool. The reader needs no database or always-running application
server. A scheduled job updates the archive and any static host can serve it.

<p>
  <img alt="The aggr reader on a desktop browser: a numbered feed of articles with their source, category, age and reading time" src="docs/images/reader-desktop.png" width="61%">
  <img alt="The same reader on a phone: the same articles, with feed, browse and preferences tabs along the bottom" src="docs/images/reader-mobile.png" width="20%">
</p>

See how one article travels from publisher to archive. Mitchell Hashimoto’s *Ghostty Is Now Non-Profit*
is available as [the original post](https://mitchellh.com/writing/ghostty-non-profit),
[a page in the live reader](https://aggr.aymericbeaumet.com/items/mitchellh-com/2025-12-03-ghostty-is-now-non-profit/),
and [the Markdown file in Git](https://github.com/aymericbeaumet/aggr-instance/blob/fe3238bc3893f17109bab9ff32ac711bc3d9c1a0/items/mitchellh-com/2025/12/2025-12-03-ghostty-is-now-non-profit.md?plain=1).

## Try locally

With [mise](https://mise.jdx.dev/) and Git installed, try the reader without creating a repository.

```sh
mise install github:aymericbeaumet/aggr@3.0.0
mise exec github:aymericbeaumet/aggr@3.0.0 -- aggr dev --demo
```

Open **http://127.0.0.1:7319**. The demo follows the two sources in the
[starter](examples/starter.toml) and keeps its files in the OS cache. Stop it with Ctrl-C.

To keep your own archive, create a project.

```sh
mkdir reads && cd reads && git init
mise use --path . github:aymericbeaumet/aggr@3.0.0
mise exec -- aggr init
# Edit aggr.toml to choose your sources.
mise exec -- aggr build
```

The build saves articles on a local `aggr` branch and writes the reader to `_site/`. With an
`origin` remote configured, it also pushes the archive. [Installation](docs/hosting.md#installing-the-binary)
covers binary downloads, source builds, and platform requirements.

## Create your reader

1. [Fork the working instance](https://github.com/aymericbeaumet/aggr-instance/fork), keeping
   **Copy the main branch only** selected to start without the demo's archive.
2. Replace `aggr.toml` with the [starter](examples/starter.toml) and edit the title and sources.
   A website URL is enough for aggr to discover a feed. Set the workflow reference in
   `.github/workflows/aggr.yml` to `aymericbeaumet/aggr/.github/workflows/aggr.yml@v3`.
3. In **Settings → Pages → Build and deployment**, set **Source** to **GitHub Actions**.
4. Open **Actions**, enable workflows, and run **aggr** on `main`. The site is
   `https://<you>.github.io/<repo>/` until you set a domain.

Edit the configuration before the first run. The starter follows two sites and keeps each feed to
250 items, 730 days, and 1 GB, using the feed text and leaving images at the publisher.
Original-page extraction and local media are settings you turn on when you want them. The default
GitHub setup publishes the reader and its archive publicly. See [sources](docs/sources.md),
[hosting](docs/hosting.md), and [build budgets](docs/build-budget.md).

For the contributor preview with fast template reload and automatic Rust restarts, follow the
mise setup in [contributing](CONTRIBUTING.md), then run `mise run dev` from this repository and
see [client development](docs/client.md).

Rust applications can use [`aggr-lib`](crates/aggr-lib/README.md), the library behind the CLI,
from a checkout or a published package. See [library setup](docs/library.md).

## How is it different?

| Project | Focus |
|---|---|
| [Miniflux](https://miniflux.app) / [FreshRSS](https://freshrss.org) | A server that reads feeds for you. aggr stores the articles as files in Git and serves a static site. |
| [Bubo](https://github.com/georgemandis/bubo-rss) | One generated page of feed links. aggr also keeps the article text. |
| [wallabag](https://wallabag.org) | Pages you save, to read later. aggr follows sources on a schedule. |
| [ArchiveBox](https://github.com/ArchiveBox/ArchiveBox) | Broad snapshots of the web, including feeds. aggr is a reader whose history is Markdown in Git. |

## The archive is a Git branch

Configuration stays on your primary branch. Articles live on a separate `aggr` branch: one
Markdown file per item, with the original URL and the time it was first captured. A sync that
changes nothing creates no commit, and one broken source does not stop the others. Items copied
from another aggr repository keep their original links. See
[copying another instance](docs/sources.md#copying-another-aggr-instance).

```text
website URL → scheduled sync → Markdown on the aggr branch → static reader
```

An open feed picks up a finished deployment on its own. [Reading](docs/reading.md) covers search,
the phone layout, keyboard shortcuts, and offline copies. A stored item and the rules for editing
the branch are in the [Git model](docs/git-model.md). How large the public demo is, and how long a
run takes, is in [benchmarks](docs/benchmarks.md).

## Limits

- Reading state stays in the browser that recorded it.
- A capture keeps the article or the feed entry. Feed text can be a summary, and original-page
  extraction can fail. Video and audio still come from the publisher.
- A public site republishes what it captured. Pages ask search engines to skip them unless you
  opt in, and that request leaves a public repository public. The original link stays on every
  item. See [publication](docs/hosting.md#publication-and-search-indexing).
- Retention limits the current tree. Older commits remain for as long as that history is kept.
  See the [Git model](docs/git-model.md).

## Documentation

[Sources](docs/sources.md) · [Reader](docs/reading.md) · [Commands](docs/commands.md) ·
[Hosting](docs/hosting.md) · [Rendering](docs/rendering.md) · [Git model](docs/git-model.md) ·
[Interoperability](docs/interoperability.md) · [Performance](docs/performance.md) ·
[Sharing a reader](docs/sharing.md) · [Releases](docs/distribution.md)

Rust prepares the archive and renders a shared Svelte application embedded in the binary.
Cargo builds the UI automatically with the native Rust toolchain. See [contributing](CONTRIBUTING.md) and
[client development](docs/client.md).

## License

[MIT](LICENSE), for every version published.
See [licensing](docs/licensing.md) for contribution terms and third-party notices.
