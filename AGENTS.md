# aggr

Git-native feed reader: a Rust CLI that turns `aggr.toml` into a static site, with every
fetched item stored on an append-only orphan git branch. Shipped as prebuilt binaries, a
composite GitHub Action (`action.yml`, install only) and a reusable workflow
(`.github/workflows/aggr.yml`) that runs sync → build → Pages deploy.

## Hard rules

- The data branch is append-only. Nothing may rewrite, squash or force-push it; every
  `blob/<sha>/…` URL must stay valid forever. Retention deletes files in normal commits only.
- A run that finds nothing new must leave no trace: no commit, no push, no state rewrite.
- Nothing is ever committed to `main` by the tool. Worktree (`.aggr/`) and output (`_site/`) go
  to `.git/info/exclude`.
- One failing source never fails the run; only config errors, git/IO errors and "every source
  failed" exit non-zero. Source errors surface through `status.toml` on transitions only.
- Stored `.html` and rendered pages are safe by construction: `content.rs` strips scripts,
  handlers and `data:`/`javascript:` URLs before storage; ammonia sanitizes before display;
  comrak renders with raw HTML off. Never serve stored HTML unsanitized.
- No data leaves the user's repository except the fetches they configured.
- Rust only. Async work uses tokio (`JoinSet` + `Semaphore`); git is shelled out; rustls with
  the ring provider, installed via `http::install_crypto_provider()` before any client (tests too).
- `config.default.toml` is the source of truth for defaults and must stay in sync with
  `config.rs` (it is embedded and parsed by a test).
- `VERSION` and `Cargo.toml` `version` must agree; releases are tags `vX.Y.Z`. The stable reusable
  workflow (`@v1`) selects the greatest published binary in its major channel at run time; binary
  releases do not move the workflow tag. The release also bumps `Formula/aggr.rb` in
  `aymericbeaumet/homebrew-tap` (the formula must exist there; the action only bumps).

## Layout

```
src/main.rs, cli.rs            entry + clap types
src/commands/*.rs              one file per subcommand; Project = config + sources + repo
src/config.rs, config/         aggr.toml types, safe local/remote include graph, ${ENV}, validation
src/git.rs                     worktree/orphan bootstrap, commit with trailers, push+rebase, refs
src/http.rs                    reqwest client: UA, timeouts, size cap, conditional GET, retries
src/sources/{mod,feed,html,aggr}.rs source dispatch, automatic feed/HTML discovery, aggr engine
src/content.rs                 strip → sanitize → Markdown; html_to_text, excerpt
src/model.rs                   front matter, dedupe keys, link normalization, file names, blob sha
src/store/                     the branch tree: items, state, seen, status, retention, front matter
src/site/                      build orchestration, template context, minijinja env, outputs
themes/default/                embedded theme (templates/, static/)
tests/cli.rs                   end-to-end: bare origin + clone + httpmock + the real binary
docs/git-model.md              the branch/ref contract; readme.md is user-facing
```

## Commands

```sh
make check                                   # fmt-check, clippy -D warnings, cargo test
cargo test --test cli                        # end-to-end only
make run ARGS="dev --port 3000"              # dogfood examples/aggr.toml
cargo run -- sync --dry-run -vv              # fetch without writing, with debug logs
```

## Conventions

- Pure functions for anything decidable (dedupe, file names, retention plans, status
  transitions, commit messages); IO at the edges in `commands/`, `store/`,
  `git.rs`. Add the unit test next to the function; add a `tests/cli.rs` scenario when git or
  the CLI surface is involved.
- `anyhow` with `.context()`; no `unwrap` outside tests.
- httpmock serves the first registered matching mock: `delete()` the old one before adding a
  new mock for the same path.
- minijinja: `trim_blocks`/`lstrip_blocks` are on, autoescape follows the `.html` extension, and
  the custom formatter escapes `& < > " '` only.
- `themes/default/static/swup.js` is the vendored Swup 4 UMD build; keep `swup.LICENSE` beside it
  and keep navigation progressively functional without JavaScript.
- A normal source URL is intentionally enough: keep HTML heuristics internal and remember the
  discovered feed endpoint. `type = "html"` and site-specific selectors are not public config.
- `sync` persists to git; `build` runs that same sync first; `dev` runs it against a namespaced
  OS cache and serves an atomic in-memory snapshot without committing or pushing.
