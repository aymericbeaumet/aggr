use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use aggr_ui_build::{BundleOptions, bundle, compile_component, compile_module, vendor_root};
use anyhow::{Context as _, Result, ensure};
use regex::Regex;
use rquickjs::{Context, Function, Runtime};
use serde_json::{Value, json};

fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry?;
        if entry.file_type().is_file() {
            result.push(entry.into_path());
        }
    }
    result.sort();
    Ok(result)
}

fn projector(root: &Path) -> Result<String> {
    let path_module = root.join("web/test/projector-path.ts");
    let mut aliases = BTreeMap::new();
    aliases.insert("path".into(), path_module.clone());
    aliases.insert(
        "svelte2tsx".into(),
        vendor_root().join("packages/svelte2tsx/index.mjs"),
    );
    aliases.insert(
        "typescript".into(),
        vendor_root().join("packages/typescript/lib/typescript.js"),
    );
    aliases.insert(
        "dedent-js".into(),
        vendor_root().join("packages/dedent-js/lib/index.js"),
    );
    aliases.insert(
        "svelte/compiler".into(),
        vendor_root().join("packages/svelte/compiler/index.js"),
    );
    Ok(bundle(
        root,
        BundleOptions {
            entry: root.join("web/test/projector.ts"),
            server: true,
            global_name: Some("AggrProjector".into()),
            aliases,
            ..Default::default()
        },
    )?
    .code)
}

fn javascript_error(ctx: &rquickjs::Ctx<'_>, error: rquickjs::Error) -> anyhow::Error {
    if error.is_exception() {
        return anyhow::anyhow!("projection failed: {:?}", ctx.catch());
    }
    error.into()
}

fn executable() -> Result<PathBuf> {
    let path = if let Some(path) = std::env::var_os("AGGR_TYPESCRIPT_BIN") {
        PathBuf::from(path)
    } else {
        let output = std::process::Command::new("mise")
            .args(["where", "http:typescript-native"])
            .output()
            .context("locating native TypeScript")?;
        ensure!(
            output.status.success(),
            "install the pinned native TypeScript compiler with mise run setup"
        );
        PathBuf::from(String::from_utf8(output.stdout)?.trim()).join(if cfg!(windows) {
            "package/lib/tsc.exe"
        } else {
            "package/lib/tsc"
        })
    };
    let mut signature = [0; 2];
    std::fs::File::open(&path)
        .with_context(|| format!("opening native TypeScript {}", path.display()))?
        .read_exact(&mut signature)?;
    ensure!(
        &signature != b"#!",
        "native TypeScript must be a binary, not a Node launcher: {}",
        path.display()
    );
    Ok(path)
}

fn source_location(map: &Value, line: usize, column: usize) -> Option<(usize, usize)> {
    let mappings = map["mappings"].as_str()?;
    let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut source = 0_i64;
    let mut original_line = 0_i64;
    let mut original_column = 0_i64;
    for (generated_line, encoded) in mappings.split(';').enumerate() {
        let mut generated_column = 0_i64;
        let mut found = None;
        for segment in encoded.split(',').filter(|s| !s.is_empty()) {
            let mut values = Vec::new();
            let mut bits = 0_u32;
            let mut value = 0_u64;
            for digit in segment.chars() {
                let number = alphabet.find(digit)? as u64;
                value |= (number & 31) << bits;
                if number & 32 != 0 {
                    bits += 5;
                } else {
                    values.push(if value & 1 == 1 {
                        -((value >> 1) as i64)
                    } else {
                        (value >> 1) as i64
                    });
                    bits = 0;
                    value = 0;
                }
            }
            generated_column += values.first()?;
            if values.len() >= 4 {
                source += values[1];
                original_line += values[2];
                original_column += values[3];
                if generated_line == line && generated_column <= column as i64 && source == 0 {
                    found = Some((original_line as usize, original_column as usize));
                }
            }
        }
        if generated_line == line {
            return found;
        }
    }
    None
}

fn report_warnings(file: &Path, warnings: &Value) -> usize {
    let Some(warnings) = warnings.as_array() else {
        return 0;
    };
    for warning in warnings {
        eprintln!(
            "{}:{}:{} warning {}: {}",
            file.display(),
            warning["start"]["line"].as_u64().unwrap_or(1),
            warning["start"]["column"].as_u64().unwrap_or(0) + 1,
            warning["code"].as_str().unwrap_or("svelte"),
            warning["message"].as_str().unwrap_or("compiler warning"),
        );
    }
    warnings.len()
}

fn check(root: &Path, binary: &Path) -> Result<()> {
    aggr_ui_model::export_types(root.join("web/.generated/types"))?;
    let directory = tempfile::tempdir()?;
    let project = directory.path().canonicalize()?;
    let project = project.as_path();
    let runtime = Runtime::new()?;
    runtime.set_memory_limit(768 * 1024 * 1024);
    runtime.set_max_stack_size(8 * 1024 * 1024);
    let context = Context::full(&runtime)?;
    let compiled = projector(root)?;
    let projector_path = root.join("web/.generated/native-check/projector.js");
    std::fs::create_dir_all(projector_path.parent().context("projector cache parent")?)?;
    std::fs::write(projector_path, &compiled)?;
    context.with(|ctx| {
        ctx.eval::<(), _>(compiled)
            .map_err(|e| javascript_error(&ctx, e))
    })?;
    let mut projections: BTreeMap<PathBuf, (PathBuf, Value)> = BTreeMap::new();
    let mut application = Vec::new();
    let mut worker = Vec::new();
    let mut warnings = 0;
    for base in ["src", "test", ".generated/types"] {
        for file in files(&root.join("web").join(base))? {
            let relative = file.strip_prefix(root.join("web"))?;
            if !matches!(
                file.extension().and_then(|s| s.to_str()),
                Some("ts" | "svelte")
            ) {
                continue;
            }
            if relative == Path::new("test/projector.ts") {
                continue;
            }
            let source = std::fs::read_to_string(&file)?;
            let target = project.join(relative);
            let target = if file.extension().is_some_and(|e| e == "svelte") {
                let output = compile_component(&source, &file.to_string_lossy(), false)?;
                warnings += report_warnings(&file, &output.warnings);
                let request = serde_json::to_string(&json!({"source":source,"filename":file}))?;
                let projected: String = context.with(|ctx| {
                    let project: Function = ctx.globals().get("aggrProject")?;
                    project
                        .call((request,))
                        .map_err(|e| javascript_error(&ctx, e))
                })?;
                let projected: Value = serde_json::from_str(&projected)?;
                let target = target.with_extension("svelte.ts");
                std::fs::create_dir_all(target.parent().context("projection parent")?)?;
                std::fs::write(
                    &target,
                    projected["code"]
                        .as_str()
                        .context("missing projected source")?,
                )?;
                projections.insert(target.clone(), (file.clone(), projected["map"].clone()));
                target
            } else {
                if file
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".svelte.ts"))
                {
                    let output = compile_module(&source, &file.to_string_lossy(), false)?;
                    warnings += report_warnings(&file, &output.warnings);
                }
                std::fs::create_dir_all(target.parent().context("source parent")?)?;
                std::fs::write(&target, source)?;
                target
            };
            if relative.starts_with("src/sw") {
                worker.push(target);
            } else {
                application.push(target);
            }
        }
    }
    let vendor = vendor_root().join("packages");
    application.extend([
        vendor.join("svelte/types/index.d.ts"),
        vendor.join("svelte2tsx/svelte-shims-v4.d.ts"),
        vendor.join("svelte2tsx/svelte-jsx-v4.d.ts"),
    ]);
    let diagnostic = Regex::new(r"^(.*)\((\d+),(\d+)\): (.*)$")?;
    let mut failed = false;
    for (name, sources, libs) in [
        ("reader", application, vec!["ES2022", "DOM", "DOM.Iterable"]),
        ("worker", worker, vec!["ES2022", "WebWorker"]),
    ] {
        if sources.is_empty() {
            continue;
        }
        let config = project.join(format!("tsconfig.{name}.json"));
        let options = json!({"compilerOptions":{"target":"ES2022","module":"ESNext","moduleResolution":"bundler","lib":libs,"strict":true,"skipLibCheck":true,"isolatedModules":true,"verbatimModuleSyntax":true,"noEmit":true,"types":[],"typeRoots":[vendor],"allowImportingTsExtensions":true,"paths":{"svelte":[vendor.join("svelte/types/index.d.ts")],"svelte/*":[vendor.join("svelte/*")],"@aggr-model/*":[project.join(".generated/types/*")]}},"files":sources});
        std::fs::write(&config, serde_json::to_vec_pretty(&options)?)?;
        let output = std::process::Command::new(binary)
            .current_dir(project)
            .args(["--noEmit", "--pretty", "false", "-p"])
            .arg(&config)
            .output()
            .with_context(|| format!("checking {name} with native TypeScript"))?;
        failed |= !output.status.success();
        for line in String::from_utf8(output.stdout)?
            .lines()
            .chain(String::from_utf8(output.stderr)?.lines())
        {
            if let Some(found) = diagnostic.captures(line) {
                let path = PathBuf::from(&found[1]);
                let path = if path.is_absolute() {
                    path
                } else {
                    project.join(path)
                };
                if let Some((original, map)) = projections.get(&path) {
                    let line = found[2].parse::<usize>()?.saturating_sub(1);
                    let column = found[3].parse::<usize>()?.saturating_sub(1);
                    if let Some((line, column)) = source_location(map, line, column) {
                        eprintln!(
                            "{}({},{}) {}",
                            original.display(),
                            line + 1,
                            column + 1,
                            &found[4]
                        );
                        continue;
                    }
                }
                if let Ok(relative) = path.strip_prefix(project) {
                    eprintln!(
                        "{}({},{}) {}",
                        root.join("web").join(relative).display(),
                        &found[2],
                        &found[3],
                        &found[4]
                    );
                    continue;
                }
            }
            eprintln!("{line}");
        }
    }
    ensure!(!failed, "native frontend type checking failed");
    println!("Svelte syntax, accessibility, CSS and TypeScript checked ({warnings} warnings)");
    Ok(())
}

pub async fn run(args: Vec<String>) -> Result<()> {
    ensure!(args.is_empty(), "usage: aggr-tools check");
    let root = crate::root().canonicalize()?;
    let binary = executable()?;
    tokio::task::spawn_blocking(move || check(&root, &binary)).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_diagnostics_map_to_original_component_positions() {
        let map = json!({"mappings":"AAAA,IAAK;AACL"});
        assert_eq!(source_location(&map, 0, 0), Some((0, 0)));
        assert_eq!(source_location(&map, 0, 4), Some((0, 5)));
        assert_eq!(source_location(&map, 1, 0), Some((1, 0)));
    }

    #[test]
    fn native_checker_validates_component_template_props() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        std::fs::create_dir_all(root.join("web/src"))?;
        std::fs::create_dir_all(root.join("web/test"))?;
        for helper in ["projector.ts", "projector-path.ts"] {
            std::fs::copy(
                crate::root().join("web/test").join(helper),
                root.join("web/test").join(helper),
            )?;
        }
        std::fs::write(
            root.join("web/src/Child.svelte"),
            "<script lang=\"ts\">let { title }: { title: string } = $props();</script><h1>{title}</h1>",
        )?;
        let parent = root.join("web/src/App.svelte");
        std::fs::write(
            &parent,
            "<script lang=\"ts\">import Child from './Child.svelte';</script><Child title={123}/>",
        )?;
        let binary = executable()?;
        ensure!(
            check(root, &binary).is_err(),
            "invalid template props passed type checking"
        );
        std::fs::write(
            parent,
            "<script lang=\"ts\">import Child from './Child.svelte';</script><Child title=\"valid\"/>",
        )?;
        check(root, &binary)
    }
}
