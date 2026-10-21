use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::{Context as _, Result, anyhow, ensure};
use rolldown::{
    Bundler, BundlerOptions, CodeSplittingMode, InputItem, OutputFormat, Platform, ResolveOptions,
};
use rolldown_common::{ModuleType, Output};
use rolldown_plugin::{
    HookLoadArgs, HookLoadOutput, HookLoadReturn, HookResolveIdArgs, HookResolveIdOutput,
    HookResolveIdReturn, HookUsage, Plugin, PluginContext, SharedLoadPluginContext,
};
use sha2::{Digest, Sha256};

use crate::{Compiled, compile_component, compile_module, vendor_root};

/// Options shared by production reader builds, native checks and browser tests.
#[derive(Debug, Clone, Default)]
pub struct BundleOptions {
    pub entry: PathBuf,
    pub server: bool,
    /// `None` emits ESM; a name emits a self-contained IIFE.
    pub global_name: Option<String>,
    pub aliases: BTreeMap<String, PathBuf>,
    /// Exact absolute module paths whose contents replace filesystem input.
    pub virtual_modules: BTreeMap<PathBuf, String>,
    pub defines: BTreeMap<String, String>,
    pub minify: bool,
    pub code_splitting: bool,
}

/// A self-contained entry and the assets produced alongside it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BundleOutput {
    pub code: String,
    pub css: Option<String>,
    pub assets: BTreeMap<String, Vec<u8>>,
    pub warnings: Vec<String>,
    pub modules: Vec<PathBuf>,
    pub entry_file: String,
    pub imports: Vec<String>,
}

fn normalized(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    normalized.pop();
                }
                component => normalized.push(component.as_os_str()),
            }
        }
        normalized
    })
}

fn optimize_css(source: &str, minify: bool) -> Result<String> {
    use lightningcss::{
        stylesheet::{MinifyOptions, ParserOptions, PrinterOptions, StyleSheet},
        targets::{Browsers, Targets},
    };
    // Lightning otherwise assumes timeline syntax inside `animation` is supported.
    // Chrome still rejects that shorthand; preserve the separate timeline longhand.
    // https://github.com/parcel-bundler/lightningcss/issues/1283
    let targets = Targets {
        browsers: Some(Browsers {
            chrome: Some(114 << 16),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut stylesheet = StyleSheet::parse(source, ParserOptions::default())
        .map_err(|error| anyhow!("parsing reader CSS: {error}"))?;
    stylesheet
        .minify(MinifyOptions {
            targets,
            ..Default::default()
        })
        .map_err(|error| anyhow!("optimizing reader CSS: {error}"))?;
    Ok(stylesheet
        .to_css(PrinterOptions {
            minify,
            targets,
            ..Default::default()
        })
        .map_err(|error| anyhow!("printing reader CSS: {error}"))?
        .code)
}

#[derive(Debug)]
struct SveltePlugin {
    root: PathBuf,
    server: bool,
    cache: PathBuf,
    virtual_modules: BTreeMap<PathBuf, String>,
    styles: Arc<Mutex<BTreeMap<String, String>>>,
    inputs: Arc<Mutex<BTreeSet<PathBuf>>>,
    aliases: BTreeMap<String, PathBuf>,
}

impl SveltePlugin {
    fn compiled(&self, id: &Path, source: &str) -> Result<Compiled> {
        let filename = id
            .strip_prefix(&self.root)
            .unwrap_or(id)
            .to_string_lossy()
            .replace('\\', "/");
        let mut digest = Sha256::new();
        digest.update(crate::compiler::fingerprint().as_bytes());
        digest.update(filename.as_bytes());
        digest.update([u8::from(self.server)]);
        digest.update(source.as_bytes());
        let key = hex::encode(digest.finalize());
        let path = self.cache.join(format!("{key}.json"));
        if let Ok(bytes) = fs::read(&path)
            && let Ok(compiled) = serde_json::from_slice(&bytes)
        {
            return Ok(compiled);
        }
        let compiled = if id
            .extension()
            .is_some_and(|extension| extension == "svelte")
        {
            compile_component(source, &filename, self.server)?
        } else {
            compile_module(source, &filename, self.server)?
        };
        fs::create_dir_all(&self.cache)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.cache)?;
        use std::io::Write as _;
        temporary.write_all(&serde_json::to_vec(&compiled)?)?;
        // Identical concurrent transforms can share the first complete cache entry.
        match temporary.persist_noclobber(&path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error.into()),
        }
        Ok(compiled)
    }
}

impl Plugin for SveltePlugin {
    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::ResolveId | HookUsage::Load
    }

    fn name(&self) -> Cow<'static, str> {
        "aggr-svelte".into()
    }

    async fn resolve_id(
        &self,
        _ctx: &PluginContext,
        args: &HookResolveIdArgs<'_>,
    ) -> HookResolveIdReturn {
        if args.specifier.ends_with("?aggr-style") {
            return Ok(Some(HookResolveIdOutput::from_id(args.specifier)));
        }
        for (alias, target) in &self.aliases {
            let path = if args.specifier == alias {
                Some(target.clone())
            } else {
                args.specifier
                    .strip_prefix(&format!("{alias}/"))
                    .map(|suffix| target.join(suffix))
            };
            if let Some(path) = path
                && (self.virtual_modules.contains_key(&path) || path.is_file())
            {
                return Ok(Some(HookResolveIdOutput::from_id(
                    normalized(&path).to_string_lossy().as_ref(),
                )));
            }
        }
        let candidate = if Path::new(args.specifier).is_absolute() {
            PathBuf::from(args.specifier)
        } else if args.specifier.starts_with('.') {
            args.importer
                .and_then(|id| Path::new(id).parent())
                .unwrap_or(&self.root)
                .join(args.specifier)
        } else {
            return Ok(None);
        };
        let candidate = normalized(&candidate);
        if self.virtual_modules.contains_key(&candidate) {
            return Ok(Some(HookResolveIdOutput::from_id(
                candidate.to_string_lossy().as_ref(),
            )));
        }
        if args.specifier.ends_with(".svelte") && !candidate.is_file() {
            let module = PathBuf::from(format!("{}.ts", candidate.display()));
            if module.is_file() || self.virtual_modules.contains_key(&module) {
                return Ok(Some(HookResolveIdOutput::from_id(
                    normalized(&module).to_string_lossy().as_ref(),
                )));
            }
        }
        Ok(None)
    }

    async fn load(&self, _ctx: SharedLoadPluginContext, args: &HookLoadArgs<'_>) -> HookLoadReturn {
        let input = PathBuf::from(args.id.split('?').next().unwrap_or(args.id));
        if input.is_file() {
            // Emitted chunks omit fully tree-shaken modules, which may later gain side effects.
            self.inputs
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(input);
        }
        if let Some(path) = args.id.strip_suffix("?aggr-style") {
            let source = match self.virtual_modules.get(Path::new(path)) {
                Some(source) => source.clone(),
                None => fs::read_to_string(path)?,
            };
            let css = self
                .compiled(Path::new(path), &source)?
                .css
                .unwrap_or_default();
            self.styles
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(args.id.into(), css);
            return Ok(Some(HookLoadOutput {
                code: "export {};".into(),
                module_type: Some(ModuleType::Js),
                ..Default::default()
            }));
        }
        if args.id.ends_with(".css") {
            let css = match self.virtual_modules.get(Path::new(args.id)) {
                Some(source) => source.clone(),
                None => fs::read_to_string(args.id)?,
            };
            self.styles
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(args.id.into(), css);
            return Ok(Some(HookLoadOutput {
                code: "export {};".into(),
                module_type: Some(ModuleType::Js),
                ..Default::default()
            }));
        }
        let id = Path::new(args.id);
        let svelte = args.id.ends_with(".svelte");
        let runes = args.id.ends_with(".svelte.ts") || args.id.ends_with(".svelte.js");
        let replacement = self.virtual_modules.get(id);
        if !svelte && !runes {
            return Ok(replacement.map(|code| HookLoadOutput {
                code: code.clone().into(),
                module_type: Some(if args.id.ends_with(".ts") {
                    ModuleType::Ts
                } else {
                    ModuleType::Js
                }),
                ..Default::default()
            }));
        }
        let source = match replacement {
            Some(source) => source.clone(),
            None => fs::read_to_string(id).with_context(|| format!("reading {}", id.display()))?,
        };
        let mut compiled = self.compiled(id, &source)?;
        if !self.server && compiled.css.as_ref().is_some_and(|css| !css.is_empty()) {
            compiled.code.push_str(&format!(
                "\nimport {};",
                serde_json::to_string(&format!("{}?aggr-style", id.display()))?
            ));
        }
        Ok(Some(HookLoadOutput {
            code: compiled.code.into(),
            module_type: Some(ModuleType::Js),
            ..Default::default()
        }))
    }
}

/// Bundle an entry without invoking Node, npm or any JavaScript build process.
/// The blocking boundary is safe to call from an existing async runtime.
pub fn bundle(root: &Path, options: BundleOptions) -> Result<BundleOutput> {
    crate::verify_vendor()?;
    let root = root.to_path_buf();
    std::thread::spawn(move || {
        static LOCK: Mutex<()> = Mutex::new(());
        static POOL: OnceLock<std::result::Result<rayon::ThreadPool, String>> = OnceLock::new();
        let _serial = LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let pool = POOL
            .get_or_init(|| {
                let capacity = std::thread::available_parallelism().map_or(1, usize::from);
                let workers = (capacity / 2).max(1).saturating_sub(1).max(1);
                rayon::ThreadPoolBuilder::new()
                    .num_threads(workers)
                    .thread_name(|index| format!("ui-bundle-{index}"))
                    .build()
                    .map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(|error| anyhow!(error.clone()))?;
        pool.install(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(bundle_async(&root, options))
        })
    })
    .join()
    .map_err(|_| anyhow!("native bundler panicked"))?
}

async fn bundle_async(root: &Path, mut options: BundleOptions) -> Result<BundleOutput> {
    let absolute = |path: &Path| {
        normalized(&if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        })
    };
    options.entry = absolute(&options.entry);
    options.aliases = options
        .aliases
        .into_iter()
        .map(|(name, path)| (name, absolute(&path)))
        .collect();
    options.virtual_modules = options
        .virtual_modules
        .into_iter()
        .map(|(path, code)| (absolute(&path), code))
        .collect();
    let mut aliases =
        BTreeMap::from([("@aggr-model".to_string(), root.join("web/.generated/types"))]);
    aliases.extend(options.aliases);
    let mut definitions = BTreeMap::from([(
        "process.env.NODE_ENV".to_string(),
        "\"production\"".to_string(),
    )]);
    definitions.extend(options.defines);
    let styles = Arc::new(Mutex::new(BTreeMap::new()));
    let inputs = Arc::new(Mutex::new(BTreeSet::new()));
    let plugin = SveltePlugin {
        root: root.into(),
        server: options.server,
        cache: root.join("web/.generated/native-transforms"),
        virtual_modules: options.virtual_modules,
        styles: Arc::clone(&styles),
        inputs: Arc::clone(&inputs),
        aliases: aliases.clone(),
    };
    let native_options = BundlerOptions {
        input: Some(vec![InputItem {
            import: options.entry.to_string_lossy().into_owned(),
            name: Some("entry".into()),
        }]),
        cwd: Some(root.into()),
        platform: Some(if options.server {
            Platform::Neutral
        } else {
            Platform::Browser
        }),
        format: Some(if options.global_name.is_some() {
            OutputFormat::Iife
        } else {
            OutputFormat::Esm
        }),
        name: options.global_name,
        code_splitting: Some(CodeSplittingMode::Bool(options.code_splitting)),
        entry_filenames: Some("app-[hash].js".to_string().into()),
        chunk_filenames: Some("[name]-[hash].js".to_string().into()),
        minify: Some(options.minify.into()),
        define: Some(definitions.into_iter().collect()),
        resolve: Some(ResolveOptions {
            alias: Some(
                aliases
                    .into_iter()
                    .map(|(name, path)| (name, vec![Some(path.to_string_lossy().into_owned())]))
                    .collect(),
            ),
            condition_names: Some(vec![
                if options.server { "worker" } else { "browser" }.into(),
                "production".into(),
                "import".into(),
                "default".into(),
            ]),
            extensions: Some(vec![
                ".ts".into(),
                ".js".into(),
                ".mjs".into(),
                ".json".into(),
                ".svelte".into(),
            ]),
            modules: Some(vec![
                vendor_root()
                    .join("packages")
                    .to_string_lossy()
                    .into_owned(),
            ]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut bundler = Bundler::with_plugins(native_options, vec![SveltePlugin::new_shared(plugin)])
        .map_err(|error| anyhow!("creating native bundle: {error:?}"))?;
    let generated = bundler
        .generate()
        .await
        .map_err(|error| anyhow!("bundling {}: {error:?}", options.entry.display()))?;
    bundler
        .close()
        .await
        .map_err(|error| anyhow!("closing native bundler: {error:?}"))?;
    let mut assets = BTreeMap::new();
    let mut code = None;
    let mut entry_file = String::new();
    let mut import_graph = BTreeMap::<String, Vec<String>>::new();
    let mut style_sources = Vec::new();
    let mut modules = inputs
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    for output in generated.assets {
        match &output {
            Output::Chunk(chunk) => {
                for id in &chunk.module_ids {
                    let path = PathBuf::from(id.as_str().split('?').next().unwrap_or(id.as_str()));
                    if path.is_file() {
                        modules.insert(path);
                    }
                }
                import_graph.insert(
                    chunk.filename.to_string(),
                    chunk.imports.iter().map(ToString::to_string).collect(),
                );
                if chunk.is_entry {
                    ensure!(
                        code.is_none(),
                        "bundle unexpectedly emitted multiple entries"
                    );
                    code = Some(chunk.code.to_string());
                    entry_file = chunk.filename.to_string();
                }
            }
            Output::Asset(asset) if asset.filename.ends_with(".css") => {
                style_sources.push(String::from_utf8(asset.source.as_bytes().to_vec())?)
            }
            _ => {}
        }
        assets.insert(
            output.filename().to_owned(),
            output.content_as_bytes().to_vec(),
        );
    }
    for (id, css) in styles
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
    {
        style_sources.push(css.clone());
        let path = PathBuf::from(id.split('?').next().unwrap_or(id));
        if path.is_file() {
            modules.insert(path);
        }
    }
    let css = if style_sources.is_empty() {
        None
    } else {
        let source = style_sources.join("\n");
        Some(optimize_css(&source, options.minify)?)
    };
    let mut imports = std::collections::BTreeSet::new();
    let mut pending = import_graph.get(&entry_file).cloned().unwrap_or_default();
    while let Some(import) = pending.pop() {
        if !imports.insert(import.clone()) {
            continue;
        }
        if let Some(children) = import_graph.get(&import) {
            pending.extend(children.iter().cloned());
        }
    }
    Ok(BundleOutput {
        entry_file,
        imports: imports.into_iter().collect(),
        code: code.context("native bundler produced no JavaScript entry")?,
        css,
        assets,
        warnings: generated
            .warnings
            .into_iter()
            .map(|warning| format!("{warning:?}"))
            .collect(),
        modules: modules.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_animations_keep_a_separate_timeline_longhand() {
        let output = optimize_css(
            r#"@property --header-progress {
                syntax: "<number>"; inherits: true; initial-value: 0;
            }
            .itemhead {
                animation: aggr-header-fold linear both;
                animation-timeline: scroll(root block);
                animation-range: 0 10rem;
            }
            @keyframes aggr-header-fold { to { --header-progress: 1; } }"#,
            true,
        )
        .unwrap();
        assert!(
            output.contains("animation-timeline:scroll(root)"),
            "{output}"
        );
        assert!(output.contains("@property --header-progress"), "{output}");
        assert!(output.contains("animation-range:0 10rem"), "{output}");
    }

    #[test]
    fn dynamic_chunks_keep_their_names_and_stay_out_of_static_preloads() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("entry.ts"),
            "export const load = () => import('./later.ts');",
        )
        .unwrap();
        fs::write(
            root.path().join("later.ts"),
            "export const later = 'Only after intent';",
        )
        .unwrap();
        let output = bundle(
            root.path(),
            BundleOptions {
                entry: root.path().join("entry.ts"),
                code_splitting: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(output.assets.len() >= 2);
        assert!(output.imports.is_empty());
        assert!(!output.code.contains("Only after intent"));
        for filename in output
            .assets
            .keys()
            .filter(|name| **name != output.entry_file)
        {
            assert!(output.code.contains(filename));
        }
    }

    #[test]
    fn shared_static_chunks_are_preloaded_but_dynamic_entries_are_not() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("entry.ts"),
            "import { shared } from './shared.ts'; export { shared }; export const load = () => import('./later.ts');",
        ).unwrap();
        fs::write(
            root.path().join("later.ts"),
            "import { shared } from './shared.ts'; export const later = shared + 'dynamic-entry-marker';",
        ).unwrap();
        fs::write(
            root.path().join("shared.ts"),
            "export const shared = Math.random();",
        )
        .unwrap();
        let output = bundle(
            root.path(),
            BundleOptions {
                entry: root.path().join("entry.ts"),
                code_splitting: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!output.imports.is_empty());
        for preload in &output.imports {
            let code = String::from_utf8_lossy(&output.assets[preload]);
            assert!(!code.contains("dynamic-entry-marker"));
            assert!(output.code.contains(preload));
        }
        assert!(output.assets.iter().any(|(name, bytes)| {
            !output.imports.contains(name)
                && String::from_utf8_lossy(bytes).contains("dynamic-entry-marker")
        }));
    }

    #[test]
    fn runes_modules_have_one_identity_through_relative_imports() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("nested")).unwrap();
        fs::write(
            root.path().join("state.svelte.ts"),
            "export const state = $state({ count: 0 });",
        )
        .unwrap();
        fs::write(root.path().join("entry.ts"), "import {state as first} from './state.svelte'; import {state as second} from './nested/../state.svelte'; export const same = first === second;").unwrap();
        let output = bundle(
            root.path(),
            BundleOptions {
                entry: root.path().join("entry.ts"),
                global_name: Some("Identity".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let runtime = rquickjs::Runtime::new().unwrap();
        let context = rquickjs::Context::full(&runtime).unwrap();
        context.with(|ctx| {
            ctx.eval::<(), _>(output.code).unwrap();
            assert!(ctx.eval::<bool, _>("Identity.same").unwrap());
        });
    }

    #[test]
    fn virtual_aliases_need_no_placeholder_files() {
        let root = tempfile::tempdir().unwrap();
        let entry = root.path().join("entry.ts");
        let dependency = root.path().join("virtual.ts");
        let output = bundle(
            root.path(),
            BundleOptions {
                entry: entry.clone(),
                aliases: BTreeMap::from([("virtual".into(), dependency.clone())]),
                virtual_modules: BTreeMap::from([
                    (entry, "export { answer } from 'virtual';".into()),
                    (dependency, "export const answer: number = 42;".into()),
                ]),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(output.code.contains("42"));
    }
}
