# aggr

Git-native feed reader: a Rust CLI that turns `aggr.toml` into a static site, with every
fetched item stored on an append-only orphan git branch. Shipped as prebuilt binaries, a
composite GitHub Action (`action.yml`, install only) and a reusable workflow
(`.github/workflows/aggr.yml`) that runs sync → build → Pages deploy → digest.

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
- No data leaves the user's repository except the fetches they configured and the GitHub API
  calls the digest makes with `GITHUB_TOKEN`.
- Rust only. Async work uses tokio (`JoinSet` + `Semaphore`); git is shelled out; rustls with
  the ring provider, installed via `http::install_crypto_provider()` before any client (tests too).
- `config.default.toml` is the source of truth for defaults and must stay in sync with
  `config.rs` (it is embedded and parsed by a test).
- `VERSION` and `Cargo.toml` `version` must agree; releases are tags `vX.Y.Z`. The release
  workflow moves the major tag (`v1`) so `@v1` follows releases, and bumps `Formula/aggr.rb` in
  `aymericbeaumet/homebrew-tap` (the formula must exist there; the action only bumps).

## Layout

```
src/main.rs, cli.rs            entry + clap types
src/commands/*.rs              one file per subcommand; Project = config + sources + repo
src/config.rs                  aggr.toml types, defaults, `include` files, ${ENV} expansion, validation
src/git.rs                     worktree/orphan bootstrap, commit with trailers, push+rebase, refs
src/http.rs                    reqwest client: UA, timeouts, size cap, conditional GET, retries
src/sources/{mod,feed,html,aggr}.rs engines; `sources::fetch` dispatches on `type`
src/content.rs                 strip → sanitize → Markdown; html_to_text, excerpt
src/model.rs                   front matter, dedupe keys, link normalization, file names, blob sha
src/store/                     the branch tree: items, state, seen, status, retention, front matter
src/site/                      build orchestration, template context, minijinja env, outputs
src/digest.rs, github.rs       digest selection/grouping/refs; the few REST calls
themes/default/                embedded theme (templates/, static/), also the digest.md template
tests/cli.rs                   end-to-end: bare origin + clone + httpmock + the real binary
docs/git-model.md              the branch/ref contract; readme.md is user-facing
```

## Commands

```sh
make check                                   # fmt-check, clippy -D warnings, cargo test
cargo test --test cli                        # end-to-end only
make run ARGS="serve --port 3000"            # dogfood examples/aggr.toml
cargo run -- sync --dry-run -vv              # fetch without writing, with debug logs
```

## Conventions

- Pure functions for anything decidable (dedupe, file names, retention plans, status
  transitions, digest selection, commit messages); IO at the edges in `commands/`, `store/`,
  `git.rs`. Add the unit test next to the function; add a `tests/cli.rs` scenario when git or
  the CLI surface is involved.
- `anyhow` with `.context()`; no `unwrap` outside tests.
- httpmock serves the first registered matching mock: `delete()` the old one before adding a
  new mock for the same path.
- minijinja: `trim_blocks`/`lstrip_blocks` are on, autoescape follows the `.html` extension, the
  custom formatter escapes `& < > " '` only; use inline `{{ x if cond }}` in `digest.md` rather
  than block tags on their own line.
