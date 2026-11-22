use anyhow::{Context as _, Result};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

fn copy(source: &Path, destination: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(source) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let target = destination.join(entry.path().strip_prefix(source)?);
        fs::create_dir_all(target.parent().context("benchmark target parent")?)?;
        fs::copy(entry.path(), target)?;
    }
    Ok(())
}

fn timed(label: &str, root: &Path, out: &Path) -> Result<f64> {
    let started = Instant::now();
    let result = aggr_ui_build::build(root, out)?;
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{label}: {:.1} ms; cache_hit={}; generation={}",
        elapsed, result.cache_hit, result.hash
    );
    Ok(elapsed)
}

fn report(label: &str, mut measurements: Vec<f64>) {
    measurements.sort_by(f64::total_cmp);
    let percentile = |value: f64| {
        measurements[((measurements.len() as f64 * value).ceil() as usize).saturating_sub(1)]
    };
    println!(
        "{label}: n={}; median={:.1} ms; p95={:.1} ms",
        measurements.len(),
        percentile(0.5),
        percentile(0.95)
    );
}

fn main() -> Result<()> {
    let source = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
    let fixture = tempfile::tempdir()?;
    for path in ["web/src", "web/public", "web/.generated/types"] {
        copy(&source.join(path), &fixture.path().join(path))?;
    }
    let out = tempfile::tempdir()?;
    timed("cold", fixture.path(), out.path())?;
    timed("unchanged", fixture.path(), out.path())?;
    let css = fixture.path().join("web/src/style.css");
    let css_source = fs::read_to_string(&css)?;
    let component = fixture.path().join("web/src/app/App.svelte");
    let component_source = fs::read_to_string(&component)?;
    anyhow::ensure!(
        component_source.contains("page-content"),
        "component fixture changed"
    );
    let mut css_times = Vec::new();
    let mut component_times = Vec::new();
    for iteration in 0..12 {
        fs::write(
            &css,
            format!("{css_source}\n.native-benchmark {{ color: rgb({iteration}, 0, 0); }}\n"),
        )?;
        css_times.push(timed("CSS edit", fixture.path(), out.path())?);
        fs::write(
            &component,
            component_source.replacen(
                "page-content",
                &format!("page-content native-benchmark-{iteration}"),
                1,
            ),
        )?;
        component_times.push(timed("component edit", fixture.path(), out.path())?);
    }
    report("CSS edits", css_times);
    report("component edits", component_times);
    timed("unchanged after edits", fixture.path(), out.path())?;
    Ok(())
}
