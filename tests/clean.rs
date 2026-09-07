//! Cleanup runs only against isolated repositories and disposable cache fixtures.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use assert_cmd::prelude::*;
use predicates::prelude::*;
use sha1::{Digest as _, Sha1};
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    cache: PathBuf,
}

impl Fixture {
    fn new(config: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("project");
        let cache = temp.path().canonicalize().unwrap().join("cache");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("aggr.toml"), config).unwrap();
        git(&root, &["add", "aggr.toml"]);
        git(&root, &["commit", "-qm", "config"]);
        Self {
            _temp: temp,
            root,
            cache,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::cargo_bin("aggr").unwrap();
        command
            .current_dir(&self.root)
            .env("AGGR_CACHE_DIR", &self.cache);
        for key in [
            "AGGR_CONFIG",
            "AGGR_BASE_URL",
            "GITHUB_REPOSITORY",
            "AGGR_CLEAN_UNSET",
        ] {
            command.env_remove(key);
        }
        command
    }

    fn dev(&self, config: &Path) -> PathBuf {
        let identity = config.to_string_lossy().replace('\\', "/");
        let hash = hex::encode(Sha1::digest(identity.as_bytes()));
        let name = slug::slugify(
            config
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap(),
        );
        self.cache
            .join("dev-v1")
            .join(format!("{name}-{}", &hash[..16]))
    }

    fn put(&self, path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn assert_no_generated_state(fixture: &Fixture) {
    assert!(
        !fixture.root.join(".aggr").exists(),
        "layout validation must run before creating the repository cache or archive worktree"
    );
    assert!(
        !fixture.root.join("_site").exists(),
        "layout validation must run before creating site output"
    );
    assert!(
        !fixture.cache.exists(),
        "layout validation must run before creating the dev cache"
    );
    assert!(
        git(&fixture.root, &["branch", "--list", "aggr"])
            .trim()
            .is_empty(),
        "layout validation must run before creating the archive branch"
    );
}

#[test]
fn ordinary_commands_reject_an_archive_inside_the_build_cache_before_writing() {
    for command in ["sync", "build", "dev"] {
        let fixture = Fixture::new("[store]\ndir = '.aggr/cache'\n");
        let mut process = fixture.command();
        process.arg(command);
        if command == "dev" {
            process.args(["--port", "0"]);
        }
        process
            .assert()
            .failure()
            .stderr(predicate::str::contains("repository build cache"));
        assert_no_generated_state(&fixture);
    }
}

#[test]
fn project_layout_rejects_every_protected_archive_overlap() {
    for (config, protected) in [
        (
            "[site]\nout = '_site'\n[store]\ndir = '_site/archive'\n",
            "site output",
        ),
        ("[store]\ndir = '.'\n", "config"),
        (
            "[site]\ntheme = 'reader-theme'\n[store]\ndir = 'reader-theme/archive'\n",
            "theme",
        ),
        ("[store]\ndir = '.git/aggr-data'\n", "Git metadata"),
    ] {
        let fixture = Fixture::new(config);
        fixture
            .command()
            .arg("sync")
            .assert()
            .failure()
            .stderr(predicate::str::contains(protected));
        assert_no_generated_state(&fixture);
    }
}

#[test]
fn dev_rejects_cache_overlaps_before_creating_its_namespace() {
    for (config, base, protected) in [
        ("[store]\ndir = 'archive'\n", "archive", "[store] dir"),
        ("", "aggr.toml", "config"),
        ("[site]\ntheme = 'reader-theme'\n", "reader-theme", "theme"),
        ("", ".git", "Git metadata"),
        ("", ".", "repository"),
    ] {
        let fixture = Fixture::new(config);
        let cache = fixture.root.join(base);
        fixture
            .command()
            .env("AGGR_CACHE_DIR", &cache)
            .args(["dev", "--port", "0"])
            .assert()
            .failure()
            .stderr(predicate::str::contains(protected));
        assert_no_generated_state(&fixture);
        assert!(
            !cache.join("dev-v1").exists(),
            "dev cache validation must happen before namespace creation"
        );
    }
}

#[cfg(unix)]
#[test]
fn ordinary_commands_reject_symlinked_archive_and_dev_cache_ancestors() {
    let archive = Fixture::new("[store]\ndir = 'linked/archive'\n");
    let outside_archive = archive.root.parent().unwrap().join("outside-archive");
    std::fs::create_dir_all(&outside_archive).unwrap();
    std::os::unix::fs::symlink(&outside_archive, archive.root.join("linked")).unwrap();
    archive
        .command()
        .arg("sync")
        .assert()
        .failure()
        .stderr(predicate::str::contains("symlink"));
    assert!(!outside_archive.join("archive").exists());
    assert_no_generated_state(&archive);

    let dev = Fixture::new("");
    let outside_cache = dev.root.parent().unwrap().join("outside-cache");
    std::fs::create_dir_all(&outside_cache).unwrap();
    std::os::unix::fs::symlink(&outside_cache, dev.root.join("linked-cache")).unwrap();
    dev.command()
        .env("AGGR_CACHE_DIR", dev.root.join("linked-cache"))
        .args(["dev", "--port", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("symlink"));
    assert!(std::fs::read_dir(&outside_cache).unwrap().next().is_none());
    assert_no_generated_state(&dev);
}

#[cfg(unix)]
#[test]
fn ordinary_commands_reject_symlinks_inside_owned_cache_namespaces() {
    let build = Fixture::new("");
    let outside_build = build.root.parent().unwrap().join("outside-build-cache");
    build.put(&outside_build.join("proof"), "keep");
    std::fs::create_dir_all(build.root.join(".aggr/cache")).unwrap();
    std::os::unix::fs::symlink(&outside_build, build.root.join(".aggr/cache/build-v1")).unwrap();
    build
        .command()
        .arg("sync")
        .assert()
        .failure()
        .stderr(predicate::str::contains("symlink"));
    assert_eq!(
        std::fs::read_to_string(outside_build.join("proof")).unwrap(),
        "keep"
    );
    assert!(
        !build.root.join(".aggr/data").exists(),
        "cache validation must precede archive bootstrap"
    );
    assert!(
        git(&build.root, &["branch", "--list", "aggr"])
            .trim()
            .is_empty()
    );

    let dev = Fixture::new("");
    let cache = dev.dev(&dev.root.join("aggr.toml"));
    let outside_dev = dev.root.parent().unwrap().join("outside-dev-data");
    dev.put(&outside_dev.join("proof"), "keep");
    std::fs::create_dir_all(&cache).unwrap();
    std::os::unix::fs::symlink(&outside_dev, cache.join("data")).unwrap();
    dev.command()
        .args(["dev", "--port", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("symlink"));
    assert_eq!(
        std::fs::read_to_string(outside_dev.join("proof")).unwrap(),
        "keep"
    );
    assert!(!cache.join("dev.lock").exists());
}

#[test]
fn repository_cache_namespace_cannot_contain_project_inputs() {
    let fixture =
        Fixture::new("[site]\ntheme = '.aggr/cache/build-v1/render-v1/previous/site/theme'\n");
    fixture.put(
        &fixture
            .root
            .join(".aggr/cache/build-v1/render-v1/previous/site/theme/templates/base.html"),
        "hand edited theme",
    );
    fixture
        .command()
        .arg("sync")
        .assert()
        .failure()
        .stderr(predicate::str::contains("repository build cache"));
    assert_eq!(
        std::fs::read_to_string(
            fixture
                .root
                .join(".aggr/cache/build-v1/render-v1/previous/site/theme/templates/base.html")
        )
        .unwrap(),
        "hand edited theme"
    );
    assert!(
        git(&fixture.root, &["branch", "--list", "aggr"])
            .trim()
            .is_empty()
    );
}

#[test]
fn repository_cache_namespace_cannot_contain_tracked_files() {
    let fixture = Fixture::new("");
    fixture.put(
        &fixture.root.join(".aggr/cache/hand-edited.toml"),
        "keep = true",
    );
    git(
        &fixture.root,
        &["add", "-f", ".aggr/cache/hand-edited.toml"],
    );
    git(&fixture.root, &["commit", "-qm", "tracked cache conflict"]);

    fixture
        .command()
        .arg("sync")
        .assert()
        .failure()
        .stderr(predicate::str::contains("tracked path"));
    assert_eq!(
        std::fs::read_to_string(fixture.root.join(".aggr/cache/hand-edited.toml")).unwrap(),
        "keep = true"
    );
    assert!(!fixture.root.join(".aggr/data").exists());
}

#[test]
fn clean_applies_the_same_repository_boundary_as_dev() {
    let fixture = Fixture::new("");
    let config = fixture.root.join("aggr.toml");
    let namespace = fixture.dev(&config).file_name().unwrap().to_os_string();
    let misplaced = fixture.root.join("dev-v1").join(namespace);
    fixture.put(&misplaced.join("data/proof"), "keep");

    fixture
        .command()
        .env("AGGR_CACHE_DIR", &fixture.root)
        .arg("clean")
        .assert()
        .failure()
        .stderr(predicate::str::contains("repository"));
    assert_eq!(
        std::fs::read_to_string(misplaced.join("data/proof")).unwrap(),
        "keep"
    );
}

#[test]
fn clean_is_offline_scoped_and_preserves_archive_and_unknown_output() {
    let fixture = Fixture::new("[[sources]]\ninclude = 'http://127.0.0.1:1/unavailable.toml'\n");
    let dev = fixture.dev(&fixture.root.join("aggr.toml"));
    let build = fixture.root.join(".aggr/cache/build-v1");
    git(&fixture.root, &["branch", "aggr"]);
    fixture.put(&build.join("articles-v1/proof"), "cached");
    fixture.put(&dev.join("data/items/proof.md"), "disposable");
    fixture.put(&dev.join("fetch/proof"), "cached");
    fixture.put(&dev.join("site/index.html"), "generated");
    fixture.put(&dev.join("site.previous/index.html"), "old generated");
    fixture.put(&dev.join("tmp/interrupted/proof"), "partial build");
    fixture.put(
        &fixture.root.join(".aggr/cache/older-build-v0/proof"),
        "stale cache",
    );
    fixture.put(
        &fixture.root.join(".aggr/data/items/proof.md"),
        "hand edited archive",
    );
    fixture.put(&fixture.root.join("_site/handmade.html"), "keep");
    let before = git(&fixture.root, &["show-ref"]);
    fixture
        .command()
        .args(["clean", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("would remove"));
    assert!(build.exists());
    assert!(!dev.join("dev.lock").exists());
    fixture.command().arg("clean").assert().success();
    assert!(!build.exists());
    assert!(!dev.join("data").exists());
    assert!(!dev.join("fetch").exists());
    assert!(!dev.join("site").exists());
    assert!(!dev.join("site.previous").exists());
    assert!(!dev.join("tmp").exists());
    assert!(
        !fixture
            .root
            .join(".aggr/cache/older-build-v0/proof")
            .exists()
    );
    assert!(fixture.root.join(".aggr/data/items/proof.md").exists());
    assert!(fixture.root.join("_site/handmade.html").exists());
    assert_eq!(git(&fixture.root, &["show-ref"]), before);
    fixture
        .command()
        .arg("clean")
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to clean"));
}

#[test]
fn clean_removes_only_marked_output_and_the_selected_config_namespace() {
    let fixture = Fixture::new("");
    fixture.put(
        &fixture.root.join("nested/aggr.toml"),
        "[site]\nout = 'generated'\n",
    );
    fixture.put(&fixture.root.join("nested/generated/.aggr-site"), "1");
    fixture.put(
        &fixture.root.join("nested/generated/index.html"),
        "generated",
    );
    fixture.put(
        &fixture
            .dev(&fixture.root.join("aggr.toml"))
            .join("data/proof"),
        "other config",
    );
    let selected = fixture.dev(&fixture.root.join("nested/aggr.toml"));
    fixture.put(&selected.join("data/proof"), "selected config");
    fixture
        .command()
        .args(["--config", "nested/aggr.toml", "clean"])
        .assert()
        .success();
    assert!(!selected.join("data").exists());
    assert!(!fixture.root.join("nested/generated").exists());
    assert!(
        fixture
            .dev(&fixture.root.join("aggr.toml"))
            .join("data/proof")
            .exists()
    );
}

#[test]
fn cleanup_preflights_every_target_and_refuses_active_dev() {
    let fixture = Fixture::new("");
    let dev = fixture.dev(&fixture.root.join("aggr.toml"));
    fixture.put(&dev.join("dev.lock"), "test");
    let lock = File::options()
        .read(true)
        .write(true)
        .open(dev.join("dev.lock"))
        .unwrap();
    lock.lock().unwrap();
    fixture.put(
        &fixture.root.join(".aggr/cache/build-v1/proof"),
        "keep while locked",
    );
    fixture
        .command()
        .arg("clean")
        .assert()
        .failure()
        .stderr(predicate::str::contains("another `aggr dev`"));
    fixture
        .command()
        .args(["clean", "--dry-run"])
        .assert()
        .failure();
    fixture
        .command()
        .args(["sync", "--clean"])
        .assert()
        .failure();
    fixture
        .command()
        .args(["build", "--clean"])
        .assert()
        .failure();
    assert!(fixture.root.join(".aggr/cache/build-v1/proof").exists());
}

#[test]
fn clean_rejects_protected_and_tracked_output() {
    for out in [".", ".git", ".aggr/data", "src", "../"] {
        let fixture = Fixture::new(&format!("[site]\nout = '{out}'\n"));
        fixture.put(&fixture.root.join("src/main.rs"), "hand edited");
        git(&fixture.root, &["add", "src/main.rs"]);
        fixture.put(
            &fixture.root.join(".aggr/cache/build-v1/proof"),
            "keep on failure",
        );
        fixture.command().arg("clean").assert().failure();
        assert!(fixture.root.join(".aggr/cache/build-v1/proof").exists());
        assert!(fixture.root.join("src/main.rs").exists());
    }
}

#[test]
fn ordinary_build_rejects_a_marked_repository_output_before_syncing() {
    let fixture = Fixture::new("[site]\nout = '.'\n");
    fixture.put(&fixture.root.join(".aggr-site"), "1");

    fixture
        .command()
        .arg("build")
        .assert()
        .failure()
        .stderr(predicate::str::contains("protected path"));

    assert!(fixture.root.join(".git").exists());
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("aggr.toml")).unwrap(),
        "[site]\nout = '.'\n"
    );
    assert!(fixture.root.join(".aggr-site").exists());
    assert!(
        !fixture.root.join(".aggr/data").exists(),
        "output safety must run before sync creates the archive worktree"
    );
}

#[cfg(unix)]
#[test]
fn cleanup_rejects_symlinks_without_touching_their_targets() {
    for inside in [false, true] {
        let fixture = Fixture::new("");
        let outside = fixture.root.parent().unwrap().join("outside");
        fixture.put(&outside.join("proof"), "keep");
        let cache = fixture.root.join(".aggr/cache/build-v1");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        let link = if inside {
            std::fs::create_dir_all(&cache).unwrap();
            cache.join("linked")
        } else {
            cache.clone()
        };
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        fixture
            .command()
            .arg("clean")
            .assert()
            .failure()
            .stderr(predicate::str::contains("symlink"));
        assert_eq!(
            std::fs::read_to_string(outside.join("proof")).unwrap(),
            "keep"
        );
        assert!(
            std::fs::symlink_metadata(link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

#[cfg(unix)]
#[test]
fn cleanup_and_build_reject_symlinked_output_roots_and_ancestors_before_sync() {
    for symlinked_ancestor in [false, true] {
        let config = if symlinked_ancestor {
            "[site]\nout = 'linked/site'\n"
        } else {
            ""
        };
        let fixture = Fixture::new(config);
        let outside = fixture.root.parent().unwrap().join("outside-output");
        let target = if symlinked_ancestor {
            outside.join("site")
        } else {
            outside.clone()
        };
        fixture.put(&target.join(".aggr-site"), "1");
        fixture.put(&target.join("proof"), "keep");
        if symlinked_ancestor {
            std::os::unix::fs::symlink(&outside, fixture.root.join("linked")).unwrap();
        } else {
            std::os::unix::fs::symlink(&outside, fixture.root.join("_site")).unwrap();
        }

        fixture
            .command()
            .arg("clean")
            .assert()
            .failure()
            .stderr(predicate::str::contains("symlink"));
        fixture
            .command()
            .arg("build")
            .assert()
            .failure()
            .stderr(predicate::str::contains("symlink"));

        assert_eq!(
            std::fs::read_to_string(target.join("proof")).unwrap(),
            "keep"
        );
        assert!(
            !fixture.root.join(".aggr/data").exists(),
            "output safety must fail before build initializes the archive"
        );
    }
}

#[test]
fn clean_dry_run_does_not_create_any_disposable_directory() {
    let fixture = Fixture::new("");
    fixture
        .command()
        .args(["clean", "--dry-run"])
        .assert()
        .success();
    assert!(!fixture.cache.exists());
    assert!(!fixture.root.join(".aggr").exists());
    assert!(!fixture.root.join("_site").exists());
}

#[test]
fn cleanup_dry_run_preserves_existing_lock_contents() {
    let fixture = Fixture::new("");
    let dev = fixture.dev(&fixture.root.join("aggr.toml"));
    fixture.put(&dev.join("dev.lock"), "old owner");
    fixture.put(&dev.join("data/proof"), "disposable");
    fixture
        .command()
        .args(["clean", "--dry-run"])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(dev.join("dev.lock")).unwrap(),
        "old owner"
    );
    assert!(dev.join("data/proof").exists());
}

#[test]
fn cleaning_only_build_artifacts_does_not_create_a_dev_namespace() {
    let fixture = Fixture::new("");
    fixture.put(&fixture.root.join(".aggr/cache/build-v1/proof"), "cached");
    fixture.put(&fixture.root.join("_site/.aggr-site"), "1");
    fixture.command().arg("clean").assert().success();
    assert!(!fixture.cache.exists());
    assert!(!fixture.root.join(".aggr/cache/build-v1").exists());
    assert!(!fixture.root.join("_site").exists());
}

#[test]
fn clean_flags_run_before_remote_include_resolution() {
    for command in ["sync", "build", "dev"] {
        let fixture = Fixture::new(
            "[fetch]\nretries = 0\n[[sources]]\ninclude = 'http://127.0.0.1:1/unavailable.toml'\n[[sources]]\nurl = '${AGGR_CLEAN_UNSET}'\n",
        );
        fixture.put(&fixture.root.join(".aggr/cache/build-v1/proof"), "cached");
        fixture
            .command()
            .args([command, "--clean"])
            .assert()
            .failure()
            .stdout(predicate::str::contains("removed"));
        assert!(!fixture.root.join(".aggr/cache/build-v1").exists());
        assert!(!fixture.root.join(".aggr/data").exists());
    }
}

#[test]
fn cleanup_protects_untracked_local_includes_and_custom_archives() {
    let included = Fixture::new("[[sources]]\ninclude = '_site/local.toml'\n");
    included.put(&included.root.join("_site/local.toml"), "");
    included.put(&included.root.join("_site/.aggr-site"), "1");
    included
        .command()
        .arg("clean")
        .assert()
        .failure()
        .stderr(predicate::str::contains("protected path"));
    assert!(included.root.join("_site/local.toml").exists());

    let custom = Fixture::new("[store]\ndir = '../archive'\n");
    let archive = custom.root.parent().unwrap().join("archive");
    custom.put(&archive.join(".aggr-site"), "1");
    custom.put(&archive.join("items/hand-edited.md"), "keep");
    custom
        .command()
        .args(["clean", "--out", archive.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("protected path"));
    assert!(archive.join("items/hand-edited.md").exists());
}

#[test]
fn cleanup_refuses_generated_output_inside_cache_namespaces() {
    let fixture = Fixture::new("[site]\nout = '.aggr/cache/build-v1/site'\n");
    fixture.put(
        &fixture.root.join(".aggr/cache/build-v1/site/.aggr-site"),
        "1",
    );
    fixture.command().arg("clean").assert().failure();
    assert!(
        fixture
            .root
            .join(".aggr/cache/build-v1/site/.aggr-site")
            .exists()
    );
}

#[test]
fn cleanup_preserves_git_metadata_even_inside_marked_output() {
    for git_dir in [".git", ".GIT"] {
        let fixture = Fixture::new("");
        fixture.put(&fixture.root.join("_site/.aggr-site"), "1");
        fixture.put(
            &fixture
                .root
                .join("_site/nested")
                .join(git_dir)
                .join("proof"),
            "keep repository",
        );
        fixture.put(
            &fixture.root.join(".aggr/cache/build-v1/proof"),
            "keep on failure",
        );
        fixture
            .command()
            .arg("clean")
            .assert()
            .failure()
            .stderr(predicate::str::contains("Git metadata"));
        assert!(
            fixture
                .root
                .join("_site/nested")
                .join(git_dir)
                .join("proof")
                .exists()
        );
        assert!(fixture.root.join(".aggr/cache/build-v1/proof").exists());
    }
}

struct RunningDev(Child);

impl Drop for RunningDev {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn dev_clean_acquires_one_lock_and_keeps_it_while_serving() {
    let fixture = Fixture::new("");
    let dev = fixture.dev(&fixture.root.join("aggr.toml"));
    fixture.put(&dev.join("data/stale-proof"), "disposable");
    fixture.put(
        &fixture.root.join(".aggr/cache/build-v1/stale-proof"),
        "cached",
    );
    let mut process = RunningDev(
        fixture
            .command()
            .args(["dev", "--clean", "--port", "0"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while !dev.join("site/.aggr-site").exists() {
        assert!(
            process.0.try_wait().unwrap().is_none(),
            "dev --clean exited before serving"
        );
        assert!(
            Instant::now() < deadline,
            "dev --clean did not publish a site"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!dev.join("data/stale-proof").exists());
    assert!(
        !fixture
            .root
            .join(".aggr/cache/build-v1/stale-proof")
            .exists()
    );
    fixture
        .command()
        .arg("clean")
        .assert()
        .failure()
        .stderr(predicate::str::contains("another `aggr dev`"));
    drop(process);
    fixture.command().arg("clean").assert().success();
}
