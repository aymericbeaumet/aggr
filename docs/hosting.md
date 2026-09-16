# Hosting an instance

aggr's storage engine uses standard Git and its output is static files, so the core is git-host and
static-host agnostic. GitHub is the turnkey path, not a requirement.

## Installing the binary

Install a prebuilt binary with
[mise's GitHub backend](https://mise.jdx.dev/dev-tools/backends/github.html):

```sh
mise use -g github:aymericbeaumet/aggr
aggr --version
```

Or download the archive for your OS and architecture from
[GitHub Releases](https://github.com/aymericbeaumet/aggr/releases/latest), verify it against the
release's `SHA256SUMS`, and put the extracted `aggr` executable on your `PATH`. Release binaries
are available for Linux, macOS, and Windows on amd64 and arm64. To select a particular release
with mise, append its version, for example `github:aymericbeaumet/aggr@1.6.0`.

## Adding aggr to an existing GitHub repository

```sh
aggr init --github
$EDITOR aggr.toml
git add aggr.toml .github/workflows/aggr.yml
git commit -m "add aggr"
git push -u origin HEAD
```

The generated workflow requests a run at minutes 7 and 37 of every hour (UTC), and after root or
nested TOML configuration and theme changes on the repository's actual default branch. Push events
from other branches are ignored; scheduled and manual runs remain available.

The `@v1` reference is a stable workflow contract. On every run it resolves the greatest published
`v1.x.y` binary, so compatible releases reach instances without a workflow edit. This reusable
workflow is deliberately a rolling major channel: pinning only its workflow tag does not also pin
the binary. A custom job can use the composite action at an immutable `@vX.Y.Z` tag with the same
exact `version` input when both pieces must be frozen.

Private repositories need a GitHub plan that supports private Pages. To build without Pages and
keep the result as a workflow artifact, add the input to the generated job:

```yaml
jobs:
  aggr:
    uses: aymericbeaumet/aggr/.github/workflows/aggr.yml@v1
    with:
      pages: false
```

## Scheduling and freshness

GitHub's scheduler can [delay or drop scheduled runs under load](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#schedule).
Avoiding the start of the hour reduces a documented source of contention, but the cron interval
is not a freshness guarantee. Artificial commits do not have a documented scheduling benefit;
aggr leaves the data branch unchanged when there is nothing new. The separate 60-day inactivity
rule can disable a public repository's schedule; check the workflow's state and
[re-enable it](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/disable-and-enable-workflows)
if needed.

For more predictable trigger timing, an external scheduler can invoke the existing manual trigger
without creating commits. For example, run this from an authenticated host every 15 or 30 minutes
(replace `OWNER/REPO` with the instance repository):

```sh
gh workflow run aggr.yml --repo OWNER/REPO
```

Give the host's repository-scoped token Actions write permission. Monitor successful deployments
and retry failed dispatches: an accepted dispatch still depends on runner availability and a
successful build. The existing GitHub schedule can remain as a fallback.

The reusable workflow already serializes sync and Pages deployment across scheduled, manual, and
push-triggered runs. Its [concurrency group](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-workflow-concurrency)
keeps one active run and one pending run; newer arrivals replace the pending run. Extra caller
concurrency or duplicate scheduled workflow files are unnecessary for that protection.

When diagnosing freshness, compare gaps between successful deployments, including the longest
gap, over complete days. Workflow creation time modulo the cron interval measures only a phase:
a five-minute delay and a 65-minute delay look identical on a 30-minute schedule. Daily run counts
show a delivery shortfall but cannot distinguish delayed events from dropped ones.

## Any Git and static host

The official releases live on GitHub, but the repository containing your config and data does not
have to. Initialize a repository on GitLab, Codeberg, Gitea, a self-hosted Git server, or another
standard Git service:

```sh
git clone https://git.example/YOU/READER.git
cd READER
aggr init
$EDITOR aggr.toml
git add aggr.toml
git commit -m "add aggr"
git push -u origin HEAD
```

Configure the provider's scheduler to run this from a normal Git checkout:

```sh
aggr build \
  --release \
  --base-url "https://reads.example.net/" \
  --out _site
```

Without `--data-ref`, `build` performs the sync first, including its commit and push, and then
renders `_site`; publish that directory with the provider's normal static-site step. The checkout
needs a writable remote named `origin`, and the server must accept `refs/heads/aggr` plus
`refs/aggr/last-good`. Set `[site] url` instead of passing `--base-url` when the public URL is
stable. Leave `[site] repository` unset outside GitHub: that field currently enables
GitHub-specific human blob/history/edit links and machine raw-config URLs only.

No GitHub API is involved in ordinary fetching, storage, rendering, or a direct HTTP(S) config
include. GitHub's reusable workflow, Pages deployment, repository shorthand, config wildcards,
and web permalinks are optional provider integrations around that portable core.

## Environment variables and exit codes

Every option has a flag; a few also read the environment so a workflow step can set them once:

| Variable | Effect |
|---|---|
| `AGGR_CONFIG` | Default for the global `--config` (otherwise `aggr.toml` in the current directory). |
| `AGGR_BASE_URL` | Default for `--base-url` on `build` and `dev`. An empty value means unset, so a skipped workflow step cannot pass the empty URL. |
| `AGGR_CACHE_DIR` | Root under which `dev` keeps its per-config cache, and where `clean` looks for it, instead of the operating system's cache directory. `sync` and `build` always use the repository-local `.aggr/cache`. |
| `AGGR_BUILD_WORKERS` | Worker threads for the CPU-bound build phases: a positive integer, clamped to 1..=8. A value that is not a positive integer is ignored with a warning, and the machine's available parallelism is used, up to eight. |

`sync` and `build` exit non-zero only for a configuration error, a git or IO error, or when every
configured source failed (`every source failed`); one failing source is reported through
`status.toml` and the run continues, so a scheduled workflow does not go red because one publisher
is down. `check` is the strict one: it probes every source and exits non-zero when any probe fails
(`N of M source(s) failed`), which makes it the right command for validating `aggr.toml` before a
merge.

## Making a public snapshot discoverable

Release builds give every published, retained item an indexable page with a self-canonical URL. The
exact upstream URL remains visibly labelled as the original and is also expressed in structured
data, feeds, the Markdown and JSON representations, and the generated `linkset.json`. Keeping the
canonical on the local page is intentional: making the upstream page canonical would ask search
engines to discard the snapshot as a duplicate.

The sitemap exposes every published, retained item. `linkset.json` gives aggr-aware tools a direct
mapping from original URLs to local copies. Site search deliberately indexes exact and normalized
original URLs alongside article prose, so a pasted URL can find a local copy on a best-effort
basis. General search engines can still choose the live original—or another copy—as the
representative result. There is no central registry and no guarantee that an unlinked instance
will be crawled, so link the public reader and submit its `sitemap.xml` through the search engines
you care about.

For a public personal or organizational archive, add truthful ownership metadata rather than
making aggr guess an identity:

```toml
[site.identity]
type = "person" # or "organization"
name = "Your Name"
url = "https://example.com/about"
same_as = ["https://github.com/you"]
```

Retention changes what remains searchable on the live site. With the default unlimited store
retention, all published captured items stay in its archives and sitemap. If `[store] max_age_days`
or `max_items` removes one, its old git object remains reachable through append-only history but
its static article page is no longer published.

See [interoperability](interoperability.md) for the exact discovery and provenance contract.
