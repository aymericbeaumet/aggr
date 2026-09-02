//! Git, shelled out. The data branch is only ever appended to: nothing here rewrites history,
//! and the one forced push (`update_ref`) moves pointer refs, never the branch.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};

const ACTIONS_BOT: (&str, &str) = (
    "github-actions[bot]",
    "41898282+github-actions[bot]@users.noreply.github.com",
);
const LOCAL_FALLBACK: (&str, &str) = ("aggr", "aggr@localhost");
const PUSH_ATTEMPTS: usize = 3;

/// The checkout holding `aggr.toml`.
#[derive(Debug)]
pub struct Repo {
    root: PathBuf,
}

/// A checkout of the data branch, linked to a [`Repo`].
#[derive(Debug)]
pub struct Worktree {
    dir: PathBuf,
    branch: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    Pushed,
    UpToDate,
    NoRemote,
}

/// Subject, free-form body lines, and trailers rendered the way `git interpret-trailers` expects
/// them: after a blank line, one `Key: value` per line, nothing after.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitMessage {
    pub subject: String,
    pub body: Vec<String>,
    pub trailers: Vec<(String, String)>,
}

impl CommitMessage {
    pub fn render(&self) -> String {
        let mut out = self.subject.trim().to_string();
        out.push('\n');
        if !self.body.is_empty() {
            out.push('\n');
            for line in &self.body {
                out.push_str(line.trim_end());
                out.push('\n');
            }
        }
        if !self.trailers.is_empty() {
            out.push('\n');
            for (key, value) in &self.trailers {
                out.push_str(&format!("{}: {}\n", key.trim(), value.trim()));
            }
        }
        out
    }
}

impl Repo {
    pub fn discover(start: &Path) -> Result<Repo> {
        let out = git(start, &["rev-parse", "--show-toplevel"])
            .with_context(|| format!("{} is not inside a git repository", start.display()))?;
        Ok(Repo {
            root: PathBuf::from(stdout(&out)),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn head_sha(&self) -> Result<Option<String>> {
        rev_parse(&self.root, "HEAD")
    }

    pub fn has_remote(&self, name: &str) -> Result<bool> {
        let out = git(&self.root, &["remote"])?;
        Ok(stdout(&out).lines().any(|line| line == name))
    }

    pub fn ensure_worktree(&self, branch: &str, dir: &Path) -> Result<Worktree> {
        let dir = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            self.root.join(dir)
        };
        let remote_ref = format!("origin/{branch}");

        if self.has_remote("origin")? {
            // The remote branch does not exist before the first sync; that is not an error.
            if git_ok(&self.root, &["fetch", "origin", branch])?.is_none() {
                log::debug!("origin/{branch} does not exist yet");
            }
        }
        let has_remote_branch = rev_parse(&self.root, &remote_ref)?.is_some();
        let has_local_branch = rev_parse(&self.root, &format!("refs/heads/{branch}"))?.is_some();

        let worktree = Worktree {
            dir: dir.clone(),
            branch: branch.to_string(),
        };
        if worktree.is_on_branch()? {
            if has_remote_branch {
                rebase_onto(&dir, &remote_ref, true)?;
            }
        } else if dir.exists() && fs::read_dir(&dir)?.next().is_some() {
            bail!(
                "{} exists but is not a worktree of branch {branch:?}; move it away or set [store] dir",
                dir.display()
            );
        } else {
            if let Some(parent) = dir.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            let dir_str = dir.to_string_lossy();
            if has_local_branch {
                git(&self.root, &["worktree", "add", &dir_str, branch])?;
            } else if has_remote_branch {
                git(
                    &self.root,
                    &[
                        "worktree",
                        "add",
                        "--track",
                        "-b",
                        branch,
                        &dir_str,
                        &remote_ref,
                    ],
                )?;
            } else {
                add_orphan_worktree(&self.root, branch, &dir_str)?;
            }
        }

        self.exclude(&dir)?;
        Ok(worktree)
    }

    /// Append the worktree directory to `.git/info/exclude` so it never shows up in `git status`
    /// on the main checkout, without touching the user's `.gitignore`.
    pub fn exclude(&self, dir: &Path) -> Result<()> {
        let Ok(rel) = dir.strip_prefix(&self.root) else {
            return Ok(());
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel.is_empty() {
            return Ok(());
        }
        let pattern = format!("/{}/", rel.trim_matches('/'));

        let out = git(&self.root, &["rev-parse", "--git-common-dir"])?;
        let common = PathBuf::from(stdout(&out));
        let common = if common.is_absolute() {
            common
        } else {
            self.root.join(common)
        };
        let exclude = common.join("info").join("exclude");
        let current = fs::read_to_string(&exclude).unwrap_or_default();
        if current.lines().any(|line| line.trim() == pattern) {
            return Ok(());
        }
        fs::create_dir_all(exclude.parent().context("exclude has a parent")?)?;
        let mut text = current;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&pattern);
        text.push('\n');
        fs::write(&exclude, text).with_context(|| format!("writing {}", exclude.display()))
    }
}

fn add_orphan_worktree(root: &Path, branch: &str, dir: &str) -> Result<()> {
    match git(root, &["worktree", "add", "--orphan", "-b", branch, dir]) {
        Ok(_) => Ok(()),
        Err(err) if format!("{err:#}").contains("unknown option") => {
            bail!("creating the {branch:?} branch needs git >= 2.42 (`git worktree add --orphan`)")
        }
        Err(err) => Err(err),
    }
}

impl Worktree {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn branch(&self) -> &str {
        &self.branch
    }

    pub fn head_sha(&self) -> Result<Option<String>> {
        rev_parse(&self.dir, "HEAD")
    }

    #[cfg(test)]
    pub fn is_dirty(&self) -> Result<bool> {
        let out = git(&self.dir, &["status", "--porcelain"])?;
        Ok(!stdout(&out).is_empty())
    }

    #[cfg(test)]
    pub fn rev_parse(&self, rev: &str) -> Result<Option<String>> {
        rev_parse(&self.dir, rev)
    }

    fn is_on_branch(&self) -> Result<bool> {
        if !self.dir.join(".git").exists() {
            return Ok(false);
        }
        let Some(out) = git_ok(&self.dir, &["symbolic-ref", "--short", "-q", "HEAD"])? else {
            return Ok(false);
        };
        Ok(stdout(&out) == self.branch)
    }

    pub fn commit(&self, message: &CommitMessage) -> Result<Option<String>> {
        git(&self.dir, &["add", "-A"])?;
        // `diff --cached --quiet` fails on an unborn branch; fall back to the index listing there.
        let staged = match self.head_sha()? {
            Some(_) => git_ok(&self.dir, &["diff", "--cached", "--quiet"])?.is_none(),
            None => !stdout(&git(&self.dir, &["ls-files", "--cached"])?).is_empty(),
        };
        if !staged {
            return Ok(None);
        }

        let identity = identity_args(&self.dir)?;
        let mut args: Vec<&str> = identity.iter().map(String::as_str).collect();
        args.extend(["commit", "-q", "-F", "-"]);
        git_stdin(&self.dir, &args, &message.render()).context("committing to the data branch")?;
        self.head_sha()
    }

    pub fn push(&self) -> Result<PushOutcome> {
        let remotes = git(&self.dir, &["remote"])?;
        if !stdout(&remotes).lines().any(|line| line == "origin") {
            return Ok(PushOutcome::NoRemote);
        }
        let remote_ref = format!("origin/{}", self.branch);

        for attempt in 1..=PUSH_ATTEMPTS {
            let out = command(&self.dir, &["push", "origin", &self.branch])
                .output()
                .context("running git push")?;
            let stderr = String::from_utf8_lossy(&out.stderr);
            if out.status.success() {
                return Ok(if stderr.contains("Everything up-to-date") {
                    PushOutcome::UpToDate
                } else {
                    PushOutcome::Pushed
                });
            }
            let rejected = stderr.contains("[rejected]")
                || stderr.contains("non-fast-forward")
                || stderr.contains("fetch first");
            if !rejected || attempt == PUSH_ATTEMPTS {
                bail!(
                    "git push origin {} failed after {attempt} attempt(s): {}",
                    self.branch,
                    stderr.trim()
                );
            }

            let shallow = git_ok(&self.dir, &["rev-parse", "--is-shallow-repository"])?
                .is_some_and(|out| stdout(&out) == "true");
            let mut fetch = vec!["fetch"];
            if shallow {
                fetch.push("--deepen=100");
            }
            fetch.extend(["origin", self.branch.as_str()]);
            git(&self.dir, &fetch)?;

            rebase_onto(&self.dir, &remote_ref, false)?;
        }
        unreachable!("push loop returns or bails")
    }

    pub fn update_ref(&self, name: &str, sha: &str) -> Result<()> {
        git(&self.dir, &["update-ref", name, sha])?;
        if self.has_origin()? {
            let refspec = format!("+{sha}:{name}");
            git(&self.dir, &["push", "-q", "origin", &refspec])
                .with_context(|| format!("pushing {name}"))?;
        }
        Ok(())
    }

    /// `(name, sha)` for every ref matching `pattern` (e.g. `refs/aggr/digest/*`), sorted by name.
    /// Reads the remote when there is one: CI checkouts never fetch `refs/aggr/*`.
    pub fn list_refs(&self, pattern: &str) -> Result<Vec<(String, String)>> {
        let mut refs: Vec<(String, String)> = if self.has_origin()? {
            let out = git(&self.dir, &["ls-remote", "--refs", "origin", pattern])?;
            stdout(&out)
                .lines()
                .filter_map(|line| {
                    let (sha, name) = line.split_once('\t')?;
                    Some((name.to_string(), sha.to_string()))
                })
                .collect()
        } else {
            let out = git(
                &self.dir,
                &["for-each-ref", "--format=%(refname) %(objectname)", pattern],
            )?;
            stdout(&out)
                .lines()
                .filter_map(|line| {
                    let (name, sha) = line.split_once(' ')?;
                    Some((name.to_string(), sha.to_string()))
                })
                .collect()
        };
        refs.sort();
        Ok(refs)
    }

    /// Make `sha` available locally, fetching it from origin if needed. GitHub allows fetching
    /// any reachable commit by sha; other hosts may not, hence the `bool`.
    pub fn ensure_commit(&self, sha: &str) -> Result<bool> {
        if rev_parse(&self.dir, sha)?.is_some() {
            return Ok(true);
        }
        if !self.has_origin()? {
            return Ok(false);
        }
        let _ = git_ok(&self.dir, &["fetch", "-q", "origin", sha]);
        Ok(rev_parse(&self.dir, sha)?.is_some())
    }

    /// Files added between two commits under `path`, in tree order.
    pub fn added_files(&self, from: &str, to: &str, path: &str) -> Result<Vec<String>> {
        let range = format!("{from}..{to}");
        let out = git(
            &self.dir,
            &[
                "diff",
                "--name-only",
                "--diff-filter=A",
                "-z",
                &range,
                "--",
                path,
            ],
        )?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// Committer time of `rev`.
    pub fn commit_time(&self, rev: &str) -> Result<chrono::DateTime<chrono::Utc>> {
        let out = git(&self.dir, &["log", "-1", "--format=%cI", rev])?;
        let text = stdout(&out);
        chrono::DateTime::parse_from_rfc3339(&text)
            .map(|date| date.with_timezone(&chrono::Utc))
            .with_context(|| format!("parsing commit time {text:?}"))
    }

    /// A detached checkout of `rev` in a temporary directory, removed on drop. For rendering the
    /// site from an older data commit (`aggr build --data-ref`).
    pub fn temp_checkout(&self, rev: &str) -> Result<TempCheckout> {
        let sha =
            rev_parse(&self.dir, rev)?.with_context(|| format!("unknown revision {rev:?}"))?;
        let tmp = tempfile::Builder::new()
            .prefix("aggr-")
            .tempdir()
            .context("creating a temporary directory")?;
        let dir = tmp.path().join("data");
        git(
            &self.dir,
            &[
                "worktree",
                "add",
                "--detach",
                "-q",
                &dir.to_string_lossy(),
                &sha,
            ],
        )?;
        Ok(TempCheckout {
            owner: self.dir.clone(),
            dir,
            sha,
            _tmp: tmp,
        })
    }

    fn has_origin(&self) -> Result<bool> {
        let remotes = git(&self.dir, &["remote"])?;
        Ok(stdout(&remotes).lines().any(|line| line == "origin"))
    }
}

pub struct TempCheckout {
    owner: PathBuf,
    dir: PathBuf,
    sha: String,
    _tmp: tempfile::TempDir,
}

impl TempCheckout {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn sha(&self) -> &str {
        &self.sha
    }
}

impl Drop for TempCheckout {
    fn drop(&mut self) {
        let dir = self.dir.to_string_lossy().into_owned();
        let _ = git_ok(&self.owner, &["worktree", "remove", "--force", &dir]);
    }
}

/// Tip of `branch` at `url` without cloning anything; `None` when the branch does not exist.
pub fn remote_tip(url: &str, branch: &str) -> Result<Option<String>> {
    let spec = format!("refs/heads/{branch}");
    let out = git(
        Path::new("."),
        &["ls-remote", "--refs", "--exit-code", url, &spec],
    );
    match out {
        Ok(out) => Ok(stdout(&out)
            .lines()
            .find_map(|line| line.split_once('\t').map(|(sha, _)| sha.to_string()))),
        // ls-remote exits 2 when the ref is missing, 128 when the remote is unreachable.
        Err(err) if format!("{err:#}").contains("(exit status: 2)") => Ok(None),
        Err(err) => Err(err),
    }
}

/// Keep a depth-1 checkout of `url`'s `branch` in `dir`, returning the tip sha. Used to read
/// another aggr repository's data branch without any history.
pub fn mirror(url: &str, branch: &str, dir: &Path) -> Result<String> {
    if dir.join(".git").exists() {
        git(dir, &["fetch", "-q", "--depth=1", "origin", branch])
            .with_context(|| format!("fetching {branch} from {url}"))?;
        git(dir, &["reset", "-q", "--hard", "FETCH_HEAD"])?;
    } else {
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        git(
            dir.parent().unwrap_or(Path::new(".")),
            &[
                "clone",
                "-q",
                "--depth=1",
                "--single-branch",
                "--branch",
                branch,
                url,
                &dir.to_string_lossy(),
            ],
        )
        .with_context(|| format!("cloning {branch} from {url}"))?;
    }
    rev_parse(dir, "HEAD")?.context("mirror has no commit")
}

/// Replay local commits on top of `onto`. During a rebase "theirs" is the commit being replayed,
/// i.e. this run's work, so regenerated files (state.toml, status.toml) keep the newest content;
/// seen.txt is union-merged through .gitattributes and never conflicts.
fn rebase_onto(dir: &Path, onto: &str, autostash: bool) -> Result<()> {
    let identity = identity_args(dir)?;
    let mut args: Vec<&str> = identity.iter().map(String::as_str).collect();
    args.push("rebase");
    if autostash {
        args.push("--autostash");
    }
    args.extend(["-X", "theirs", onto]);
    if git_ok(dir, &args)?.is_none() {
        let _ = git_ok(dir, &["rebase", "--abort"]);
        bail!(
            "the data branch diverged from {onto} and could not be rebased; resolve manually in {}",
            dir.display()
        );
    }
    Ok(())
}

/// `-c user.name=… -c user.email=…` for commands that create commits. Git refuses to commit or
/// rebase without an identity, and CI checkouts have none configured; nothing is written to config.
fn identity_args(dir: &Path) -> Result<Vec<String>> {
    let name = git_ok(dir, &["config", "user.name"])?.map(|out| stdout(&out));
    let email = git_ok(dir, &["config", "user.email"])?.map(|out| stdout(&out));
    let (name, email) = match (name, email) {
        (Some(name), Some(email)) if !name.is_empty() && !email.is_empty() => (name, email),
        _ => {
            let (name, email) = if std::env::var_os("GITHUB_ACTIONS").is_some() {
                ACTIONS_BOT
            } else {
                LOCAL_FALLBACK
            };
            (name.to_string(), email.to_string())
        }
    };
    Ok(vec![
        "-c".into(),
        format!("user.name={name}"),
        "-c".into(),
        format!("user.email={email}"),
    ])
}

fn rev_parse(dir: &Path, rev: &str) -> Result<Option<String>> {
    let spec = format!("{rev}^{{commit}}");
    Ok(git_ok(dir, &["rev-parse", "--verify", "-q", &spec])?.map(|out| stdout(&out)))
}

fn command(dir: &Path, args: &[&str]) -> Command {
    log::debug!("git {} (in {})", args.join(" "), dir.display());
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    #[cfg(test)]
    {
        cmd.env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
    }
    cmd
}

/// Run git and fail on a non-zero exit, with the command and stderr in the error.
fn git(dir: &Path, args: &[&str]) -> Result<Output> {
    let out = command(dir, args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    check(out, args)
}

/// Run git, returning `None` on a non-zero exit. For probes where failure is an answer.
fn git_ok(dir: &Path, args: &[&str]) -> Result<Option<Output>> {
    let out = command(dir, args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    Ok(out.status.success().then_some(out))
}

fn git_stdin(dir: &Path, args: &[&str], input: &str) -> Result<Output> {
    let mut child = command(dir, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    child
        .stdin
        .take()
        .context("git stdin")?
        .write_all(input.as_bytes())
        .context("writing to git stdin")?;
    let out = child.wait_with_output().context("waiting for git")?;
    check(out, args)
}

fn check(out: Output, args: &[&str]) -> Result<Output> {
    if out.status.success() {
        return Ok(out);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stderr = stderr.trim();
    let detail = if stderr.is_empty() {
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    } else {
        stderr.to_string()
    };
    bail!("git {} failed ({}): {detail}", args.join(" "), out.status)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sh(dir: &Path, args: &[&str]) -> String {
        stdout(&git(dir, args).unwrap_or_else(|err| panic!("{err:#}")))
    }

    fn identity() -> [&'static str; 4] {
        ["-c", "user.name=t", "-c", "user.email=t@t"]
    }

    fn commit_all(dir: &Path, msg: &str) {
        let mut args = identity().to_vec();
        args.extend(["commit", "-q", "-m", msg]);
        git(dir, &["add", "-A"]).unwrap();
        git(dir, &args).unwrap();
    }

    /// Bare origin + a clone with one commit on `main`.
    fn fixture() -> (TempDir, Repo) {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        git(
            tmp.path(),
            &["init", "-q", "--bare", "-b", "main", "origin.git"],
        )
        .unwrap();
        let clone = tmp.path().join("clone");
        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        )
        .unwrap();
        fs::write(clone.join("aggr.toml"), "[site]\n").unwrap();
        commit_all(&clone, "init");
        git(&clone, &["push", "-q", "-u", "origin", "main"]).unwrap();
        let repo = Repo::discover(&clone).unwrap();
        (tmp, repo)
    }

    fn local_fixture(with_commit: bool) -> (TempDir, Repo) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("repo");
        git(tmp.path(), &["init", "-q", "-b", "main", "repo"]).unwrap();
        if with_commit {
            fs::write(root.join("aggr.toml"), "[site]\n").unwrap();
            commit_all(&root, "init");
        }
        let repo = Repo::discover(&root).unwrap();
        (tmp, repo)
    }

    fn second_clone(tmp: &TempDir, branch: &str) -> PathBuf {
        let origin = tmp.path().join("origin.git");
        let other = tmp.path().join("other");
        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                "-b",
                branch,
                origin.to_str().unwrap(),
                other.to_str().unwrap(),
            ],
        )
        .unwrap();
        other
    }

    fn origin_file(tmp: &TempDir, branch: &str, path: &str) -> Option<String> {
        let origin = tmp.path().join("origin.git");
        git_ok(&origin, &["show", &format!("{branch}:{path}")])
            .unwrap()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn ls_remote(tmp: &TempDir) -> String {
        sh(&tmp.path().join("origin.git"), &["ls-remote", "."])
    }

    #[test]
    fn renders_commit_messages() {
        let plain = CommitMessage {
            subject: "aggr: +3 items".into(),
            ..Default::default()
        };
        assert_eq!(plain.render(), "aggr: +3 items\n");

        let full = CommitMessage {
            subject: "aggr: +3 items".into(),
            body: vec!["rust-blog: +2".into(), "hn: +1".into()],
            trailers: vec![
                ("Aggr-Version".into(), "0.1.0".into()),
                ("Aggr-Sources".into(), "2 ok, 0 error".into()),
            ],
        };
        assert_eq!(
            full.render(),
            "aggr: +3 items\n\nrust-blog: +2\nhn: +1\n\nAggr-Version: 0.1.0\nAggr-Sources: 2 ok, 0 error\n"
        );

        let trailers_only = CommitMessage {
            subject: "aggr: init".into(),
            trailers: vec![("Aggr-Version".into(), "0.1.0".into())],
            ..Default::default()
        };
        assert_eq!(
            trailers_only.render(),
            "aggr: init\n\nAggr-Version: 0.1.0\n"
        );
    }

    #[test]
    fn discovers_root_and_head() {
        let (_tmp, repo) = fixture();
        assert!(repo.root().join("aggr.toml").exists());
        assert!(repo.head_sha().unwrap().is_some());
        assert!(repo.has_remote("origin").unwrap());
        assert!(!repo.has_remote("upstream").unwrap());
        let sub = repo.root().join("sub");
        fs::create_dir(&sub).unwrap();
        assert_eq!(Repo::discover(&sub).unwrap().root(), repo.root());
        assert!(Repo::discover(_tmp.path()).is_err());
    }

    #[test]
    fn orphan_worktree_is_created_and_reused() {
        let (_tmp, repo) = fixture();
        let dir = Path::new(".aggr/data");
        let wt = repo.ensure_worktree("aggr", dir).unwrap();
        assert_eq!(wt.dir(), repo.root().join(dir));
        assert_eq!(wt.branch(), "aggr");
        assert_eq!(wt.head_sha().unwrap(), None);
        assert!(!wt.is_dirty().unwrap());

        let exclude = repo.root().join(".git/info/exclude");
        let count = |text: &str| text.lines().filter(|l| *l == "/.aggr/data/").count();
        assert_eq!(count(&fs::read_to_string(&exclude).unwrap()), 1);

        let again = repo.ensure_worktree("aggr", dir).unwrap();
        assert_eq!(again.dir(), wt.dir());
        assert_eq!(count(&fs::read_to_string(&exclude).unwrap()), 1);
        assert_eq!(sh(repo.root(), &["status", "--porcelain"]), "");
    }

    #[test]
    fn commit_returns_sha_then_none() {
        let (_tmp, repo) = fixture();
        let wt = repo
            .ensure_worktree("aggr", Path::new(".aggr/data"))
            .unwrap();
        let msg = CommitMessage {
            subject: "aggr: init".into(),
            trailers: vec![("Aggr-Version".into(), "0.1.0".into())],
            ..Default::default()
        };
        assert_eq!(wt.commit(&msg).unwrap(), None);

        fs::write(wt.dir().join("README.md"), "hi\n").unwrap();
        assert!(wt.is_dirty().unwrap());
        let sha = wt.commit(&msg).unwrap().expect("a commit");
        assert_eq!(wt.head_sha().unwrap().as_deref(), Some(sha.as_str()));
        assert!(!wt.is_dirty().unwrap());
        assert_eq!(wt.commit(&msg).unwrap(), None);

        let body = sh(wt.dir(), &["log", "-1", "--format=%B"]);
        assert!(body.contains("Aggr-Version: 0.1.0"), "{body}");
        let trailers = sh(
            wt.dir(),
            &[
                "log",
                "-1",
                "--format=%(trailers:key=Aggr-Version,valueonly)",
            ],
        );
        assert_eq!(trailers, "0.1.0");
        let author = sh(wt.dir(), &["log", "-1", "--format=%an <%ae>"]);
        assert_eq!(author, "aggr <aggr@localhost>");
    }

    #[test]
    fn commit_uses_configured_identity() {
        let (_tmp, repo) = fixture();
        git(repo.root(), &["config", "user.name", "Someone"]).unwrap();
        git(repo.root(), &["config", "user.email", "s@example.com"]).unwrap();
        let wt = repo
            .ensure_worktree("aggr", Path::new(".aggr/data"))
            .unwrap();
        fs::write(wt.dir().join("README.md"), "hi\n").unwrap();
        wt.commit(&CommitMessage {
            subject: "x".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            sh(wt.dir(), &["log", "-1", "--format=%an <%ae>"]),
            "Someone <s@example.com>"
        );
    }

    #[test]
    fn pushes_then_reports_up_to_date() {
        let (tmp, repo) = fixture();
        let wt = repo
            .ensure_worktree("aggr", Path::new(".aggr/data"))
            .unwrap();
        fs::write(wt.dir().join("README.md"), "hi\n").unwrap();
        let sha = wt
            .commit(&CommitMessage {
                subject: "aggr: init".into(),
                ..Default::default()
            })
            .unwrap()
            .unwrap();
        assert_eq!(wt.push().unwrap(), PushOutcome::Pushed);
        assert_eq!(wt.push().unwrap(), PushOutcome::UpToDate);
        assert!(ls_remote(&tmp).contains(&format!("{sha}\trefs/heads/aggr")));
    }

    #[test]
    fn existing_remote_branch_is_checked_out_and_updated() {
        let (tmp, repo) = fixture();
        let wt = repo
            .ensure_worktree("aggr", Path::new(".aggr/data"))
            .unwrap();
        fs::write(wt.dir().join("README.md"), "hi\n").unwrap();
        wt.commit(&CommitMessage {
            subject: "aggr: init".into(),
            ..Default::default()
        })
        .unwrap();
        wt.push().unwrap();

        // A fresh clone (what CI sees) picks up the remote branch.
        let fresh = tmp.path().join("fresh");
        git(
            tmp.path(),
            &[
                "clone",
                "-q",
                tmp.path().join("origin.git").to_str().unwrap(),
                fresh.to_str().unwrap(),
            ],
        )
        .unwrap();
        let fresh_repo = Repo::discover(&fresh).unwrap();
        let fresh_wt = fresh_repo
            .ensure_worktree("aggr", Path::new(".aggr/data"))
            .unwrap();
        assert_eq!(fresh_wt.head_sha().unwrap(), wt.head_sha().unwrap());
        assert!(fresh_wt.dir().join("README.md").exists());

        // Someone else pushes; the persistent local worktree fast-forwards on the next ensure.
        let other = second_clone(&tmp, "aggr");
        fs::write(other.join("more.txt"), "x\n").unwrap();
        commit_all(&other, "more");
        git(&other, &["push", "-q", "origin", "aggr"]).unwrap();
        let wt = repo
            .ensure_worktree("aggr", Path::new(".aggr/data"))
            .unwrap();
        assert!(wt.dir().join("more.txt").exists());
    }

    #[test]
    fn push_rebases_on_rejection() {
        let (tmp, repo) = fixture();
        let wt = repo
            .ensure_worktree("aggr", Path::new(".aggr/data"))
            .unwrap();
        fs::write(wt.dir().join("README.md"), "hi\n").unwrap();
        wt.commit(&CommitMessage {
            subject: "aggr: init".into(),
            ..Default::default()
        })
        .unwrap();
        wt.push().unwrap();

        let other = second_clone(&tmp, "aggr");
        fs::write(other.join("theirs.txt"), "theirs\n").unwrap();
        commit_all(&other, "theirs");
        git(&other, &["push", "-q", "origin", "aggr"]).unwrap();

        fs::write(wt.dir().join("ours.txt"), "ours\n").unwrap();
        wt.commit(&CommitMessage {
            subject: "ours".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(wt.push().unwrap(), PushOutcome::Pushed);
        assert_eq!(
            origin_file(&tmp, "aggr", "ours.txt").as_deref(),
            Some("ours\n")
        );
        assert_eq!(
            origin_file(&tmp, "aggr", "theirs.txt").as_deref(),
            Some("theirs\n")
        );
        assert_eq!(sh(wt.dir(), &["rev-list", "--count", "HEAD"]), "3");
    }

    #[test]
    fn seen_lines_union_merge_on_rebase() {
        let (tmp, repo) = fixture();
        let wt = repo
            .ensure_worktree("aggr", Path::new(".aggr/data"))
            .unwrap();
        fs::write(
            wt.dir().join(".gitattributes"),
            "sources/*/seen.txt merge=union\n",
        )
        .unwrap();
        fs::create_dir_all(wt.dir().join("sources/x")).unwrap();
        fs::write(wt.dir().join("sources/x/seen.txt"), "k0 2026-01-01\n").unwrap();
        wt.commit(&CommitMessage {
            subject: "aggr: init".into(),
            ..Default::default()
        })
        .unwrap();
        wt.push().unwrap();

        let other = second_clone(&tmp, "aggr");
        fs::write(
            other.join("sources/x/seen.txt"),
            "k0 2026-01-01\nk1 2026-09-01\n",
        )
        .unwrap();
        commit_all(&other, "theirs");
        git(&other, &["push", "-q", "origin", "aggr"]).unwrap();

        fs::write(
            wt.dir().join("sources/x/seen.txt"),
            "k0 2026-01-01\nk2 2026-09-02\n",
        )
        .unwrap();
        wt.commit(&CommitMessage {
            subject: "ours".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(wt.push().unwrap(), PushOutcome::Pushed);
        let seen = origin_file(&tmp, "aggr", "sources/x/seen.txt").unwrap();
        assert!(seen.contains("k1 2026-09-01"), "{seen}");
        assert!(seen.contains("k2 2026-09-02"), "{seen}");
        assert_eq!(seen.matches("k0").count(), 1, "{seen}");
    }

    #[test]
    fn conflicting_regenerated_file_keeps_ours() {
        let (tmp, repo) = fixture();
        let wt = repo
            .ensure_worktree("aggr", Path::new(".aggr/data"))
            .unwrap();
        fs::write(wt.dir().join("status.toml"), "a = 1\n").unwrap();
        wt.commit(&CommitMessage {
            subject: "aggr: init".into(),
            ..Default::default()
        })
        .unwrap();
        wt.push().unwrap();

        let other = second_clone(&tmp, "aggr");
        fs::write(other.join("status.toml"), "a = 2\n").unwrap();
        commit_all(&other, "theirs");
        git(&other, &["push", "-q", "origin", "aggr"]).unwrap();

        fs::write(wt.dir().join("status.toml"), "a = 3\n").unwrap();
        wt.commit(&CommitMessage {
            subject: "ours".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(wt.push().unwrap(), PushOutcome::Pushed);
        assert_eq!(
            origin_file(&tmp, "aggr", "status.toml").as_deref(),
            Some("a = 3\n")
        );
    }

    #[test]
    fn update_ref_is_pushed() {
        let (tmp, repo) = fixture();
        let wt = repo
            .ensure_worktree("aggr", Path::new(".aggr/data"))
            .unwrap();
        fs::write(wt.dir().join("README.md"), "hi\n").unwrap();
        let sha = wt
            .commit(&CommitMessage {
                subject: "aggr: init".into(),
                ..Default::default()
            })
            .unwrap()
            .unwrap();
        wt.push().unwrap();
        wt.update_ref("refs/aggr/last-good", &sha).unwrap();
        assert!(ls_remote(&tmp).contains(&format!("{sha}\trefs/aggr/last-good")));
        assert_eq!(
            wt.rev_parse("refs/aggr/last-good").unwrap().as_deref(),
            Some(sha.as_str())
        );
        assert_eq!(wt.rev_parse("refs/aggr/nope").unwrap(), None);

        // Pointers move freely, including backwards.
        fs::write(wt.dir().join("b.txt"), "b\n").unwrap();
        let sha2 = wt
            .commit(&CommitMessage {
                subject: "b".into(),
                ..Default::default()
            })
            .unwrap()
            .unwrap();
        wt.update_ref("refs/aggr/last-good", &sha2).unwrap();
        wt.update_ref("refs/aggr/last-good", &sha).unwrap();
        assert!(ls_remote(&tmp).contains(&format!("{sha}\trefs/aggr/last-good")));
    }

    #[test]
    fn no_remote_is_reported() {
        let (_tmp, repo) = local_fixture(true);
        assert!(!repo.has_remote("origin").unwrap());
        let wt = repo
            .ensure_worktree("aggr", Path::new(".aggr/data"))
            .unwrap();
        fs::write(wt.dir().join("README.md"), "hi\n").unwrap();
        let sha = wt
            .commit(&CommitMessage {
                subject: "aggr: init".into(),
                ..Default::default()
            })
            .unwrap()
            .unwrap();
        assert_eq!(wt.push().unwrap(), PushOutcome::NoRemote);
        wt.update_ref("refs/aggr/last-good", &sha).unwrap();
        assert_eq!(
            wt.rev_parse("refs/aggr/last-good").unwrap().as_deref(),
            Some(sha.as_str())
        );
    }

    #[test]
    fn unborn_main_still_gets_a_worktree() {
        let (_tmp, repo) = local_fixture(false);
        assert_eq!(repo.head_sha().unwrap(), None);
        let wt = repo
            .ensure_worktree("aggr", Path::new(".aggr/data"))
            .unwrap();
        assert_eq!(wt.head_sha().unwrap(), None);
        fs::write(wt.dir().join("README.md"), "hi\n").unwrap();
        assert!(
            wt.commit(&CommitMessage {
                subject: "aggr: init".into(),
                ..Default::default()
            })
            .unwrap()
            .is_some()
        );
        assert_eq!(
            sh(repo.root(), &["branch", "--list", "aggr"])
                .trim_start_matches('+')
                .trim(),
            "aggr"
        );
    }

    #[test]
    fn absolute_dir_outside_root_is_not_excluded() {
        let (tmp, repo) = local_fixture(true);
        let outside = tmp.path().join("elsewhere");
        let wt = repo.ensure_worktree("aggr", &outside).unwrap();
        assert_eq!(wt.dir(), outside);
        let exclude = fs::read_to_string(repo.root().join(".git/info/exclude")).unwrap_or_default();
        assert!(!exclude.contains("elsewhere"), "{exclude}");
    }

    #[test]
    fn foreign_directory_is_refused() {
        let (_tmp, repo) = local_fixture(true);
        let dir = repo.root().join(".aggr/data");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("stray.txt"), "x").unwrap();
        let err = repo
            .ensure_worktree("aggr", Path::new(".aggr/data"))
            .unwrap_err();
        assert!(format!("{err:#}").contains("not a worktree"), "{err:#}");
    }
}
