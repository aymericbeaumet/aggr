//! The compiled Svelte reader and its isolated synchronous JavaScript renderer.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aggr_ui_model::{ClientAssets, RenderRequest};
use anyhow::{Context as _, Result, anyhow};
use rquickjs::{Context, Ctx, Function, Runtime};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

include!(concat!(env!("OUT_DIR"), "/ui.rs"));

pub fn implementation_hash() -> &'static str {
    env!("AGGR_UI_HASH")
}

#[cfg(test)]
fn embedded(name: &str) -> Result<&'static [u8]> {
    UI_FILES
        .iter()
        .find(|(path, _)| *path == name)
        .map(|(_, bytes)| *bytes)
        .with_context(|| format!("missing embedded UI asset {name}"))
}

#[derive(Debug, Deserialize)]
struct Manifest {
    contract: String,
    app: String,
    css: Option<String>,
    imports: Vec<String>,
    bootstrap: String,
    worker: String,
    files: BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct Renderer {
    manifest: Arc<Manifest>,
    bundle: Arc<UiBundle>,
}

struct UiBundle {
    hash: String,
    renderer_hash: String,
    files: BTreeMap<String, Arc<[u8]>>,
}

fn valid_asset(path: &str) -> bool {
    path.starts_with("assets/")
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains('\\'))
}

impl Renderer {
    pub fn new() -> Result<Self> {
        if let Some(root) = std::env::var_os("AGGR_UI_DIR") {
            return Self::from_current(Path::new(&root));
        }
        Self::from_files(
            UI_FILES
                .iter()
                .map(|(name, bytes)| ((*name).to_owned(), Arc::from(*bytes)))
                .collect(),
        )
    }

    /// Load the pointer once, then retain all bytes from that immutable generation.
    pub fn from_current(root: &Path) -> Result<Self> {
        #[derive(Deserialize)]
        struct Current {
            directory: String,
        }
        let current: Current = serde_json::from_slice(&std::fs::read(root.join("current.json"))?)
            .context("reading active reader generation")?;
        anyhow::ensure!(
            !current.directory.is_empty()
                && current
                    .directory
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()),
            "invalid reader generation directory"
        );
        Self::from_directory(&root.join(current.directory))
    }

    pub fn from_directory(root: &Path) -> Result<Self> {
        let manifest_bytes = std::fs::read(root.join("assets.json"))?;
        let manifest: Manifest = serde_json::from_slice(&manifest_bytes)?;
        let mut files = BTreeMap::from([
            ("assets.json".to_owned(), Arc::from(manifest_bytes)),
            (
                "renderer.js".to_owned(),
                Arc::from(std::fs::read(root.join("renderer.js"))?),
            ),
        ]);
        for path in manifest.files.values() {
            anyhow::ensure!(valid_asset(path), "invalid UI asset path {path}");
            files.insert(path.clone(), Arc::from(std::fs::read(root.join(path))?));
        }
        Self::from_files(files)
    }

    fn from_files(files: BTreeMap<String, Arc<[u8]>>) -> Result<Self> {
        let manifest: Manifest = serde_json::from_slice(
            files
                .get("assets.json")
                .context("missing reader manifest")?,
        )?;
        anyhow::ensure!(
            manifest.contract == env!("AGGR_UI_CONTRACT_HASH"),
            "reader bundle uses a different page-model contract; rebuild the Rust worker"
        );
        for path in manifest.files.values() {
            anyhow::ensure!(valid_asset(path), "invalid UI asset path {path}");
            anyhow::ensure!(files.contains_key(path), "missing reader asset {path}");
        }
        for path in std::iter::once(&manifest.app)
            .chain(manifest.css.iter())
            .chain(manifest.imports.iter())
            .chain([&manifest.bootstrap, &manifest.worker])
        {
            anyhow::ensure!(
                manifest.files.values().any(|value| value == path),
                "unlisted reader entry {path}"
            );
        }
        let source = files
            .get("renderer.js")
            .context("missing reader renderer")?;
        Javascript::from_source(source).context("validating reader renderer")?;
        let renderer_hash = hex::encode(Sha256::digest(source));
        let mut hash = Sha256::new();
        for (name, bytes) in &files {
            hash.update((name.len() as u64).to_le_bytes());
            hash.update(name.as_bytes());
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        Ok(Self {
            manifest: Arc::new(manifest),
            bundle: Arc::new(UiBundle {
                hash: hex::encode(hash.finalize()),
                renderer_hash,
                files,
            }),
        })
    }

    #[cfg(test)]
    pub(super) fn test_source(source: &str) -> Self {
        let mut files: BTreeMap<String, Arc<[u8]>> = UI_FILES
            .iter()
            .map(|(name, bytes)| ((*name).to_owned(), Arc::from(*bytes)))
            .collect();
        files.insert("renderer.js".into(), Arc::from(source.as_bytes()));
        Self::from_files(files).unwrap()
    }

    pub fn hash(&self) -> &str {
        &self.bundle.hash
    }

    pub fn client_assets(&self) -> ClientAssets {
        ClientAssets {
            app: Some(self.manifest.app.clone()),
            css: self.manifest.css.clone(),
            imports: self.manifest.imports.clone(),
            icon: self.asset_site_path("assets/favicon-32.png"),
        }
    }

    pub fn asset_site_path(&self, path: &str) -> String {
        match path {
            "assets/app/bootstrap.js" => self.manifest.bootstrap.clone(),
            "assets/app/worker.js" => self.manifest.worker.clone(),
            _ => self
                .manifest
                .files
                .get(path)
                .cloned()
                .unwrap_or_else(|| path.to_string()),
        }
    }

    pub fn render_page(&self, request: &RenderRequest) -> Result<String> {
        let json = serde_json::to_string(request).context("serializing reader page")?;
        self.render_json(&json)
            .with_context(|| format!("rendering {}", request.page.path))
    }

    fn render_json(&self, json: &str) -> Result<String> {
        RENDERER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot
                .as_ref()
                .is_none_or(|(hash, _)| hash != &self.bundle.renderer_hash)
            {
                let source = self
                    .bundle
                    .files
                    .get("renderer.js")
                    .context("missing reader renderer")?;
                *slot = Some((
                    self.bundle.renderer_hash.clone(),
                    Javascript::from_source(source)?,
                ));
            }
            let result = slot
                .as_ref()
                .context("initializing reader renderer")?
                .1
                .render(json);
            if result.is_err() {
                // An interrupted or exhausted context cannot affect the following page.
                *slot = None;
            }
            result
        })
    }

    pub fn write_static(&self, out: &Path) -> Result<Vec<String>> {
        let mut paths: Vec<_> = self.manifest.files.values().cloned().collect();
        paths.sort();
        paths.dedup();
        for path in &paths {
            let destination = out.join(path);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(
                &destination,
                self.bundle
                    .files
                    .get(path)
                    .context("missing reader asset")?,
            )
            .with_context(|| format!("writing {}", destination.display()))?;
        }
        Ok(paths
            .into_iter()
            .map(|path| path.trim_start_matches("assets/").to_string())
            .collect())
    }

    pub fn eager_assets(&self) -> Vec<String> {
        let mut paths = vec![
            self.manifest.app.clone(),
            self.manifest.bootstrap.clone(),
            self.manifest.worker.clone(),
        ];
        paths.extend(self.manifest.css.iter().cloned());
        paths.extend(self.manifest.imports.iter().cloned());
        paths.sort();
        paths.dedup();
        paths
    }
}

thread_local! {
    static RENDERER: RefCell<Option<(String, Javascript)>> = const { RefCell::new(None) };
}

struct Javascript {
    context: Context,
    deadline: Rc<Cell<Option<Instant>>>,
    _runtime: Runtime,
}

impl Javascript {
    #[cfg(test)]
    fn new() -> Result<Self> {
        Self::from_source(embedded("renderer.js")?)
    }

    fn from_source(source: &[u8]) -> Result<Self> {
        let runtime = Runtime::new().context("creating reader JavaScript runtime")?;
        runtime.set_memory_limit(128 * 1024 * 1024);
        runtime.set_max_stack_size(1024 * 1024);
        let deadline: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        let timeout = deadline.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            timeout.get().is_some_and(|end| Instant::now() >= end)
        })));
        let context = Context::full(&runtime).context("creating reader JavaScript context")?;
        deadline.set(Some(Instant::now() + Duration::from_secs(10)));
        context.with(|ctx| {
            ctx.eval::<(), _>(source)
                .map_err(|error| javascript_error(&ctx, error))?;
            ctx.globals()
                .get::<_, Function>("aggrRender")
                .map(|_| ())
                .map_err(|error| javascript_error(&ctx, error))
        })?;
        deadline.set(None);
        Ok(Self {
            context,
            deadline,
            _runtime: runtime,
        })
    }

    fn render(&self, json: &str) -> Result<String> {
        self.deadline
            .set(Some(Instant::now() + Duration::from_secs(10)));
        let result = self.context.with(|ctx| {
            let render: Function = ctx
                .globals()
                .get("aggrRender")
                .map_err(|error| javascript_error(&ctx, error))?;
            render
                .call::<_, String>((json,))
                .map_err(|error| javascript_error(&ctx, error))
        });
        self.deadline.set(None);
        result
    }
}

fn javascript_error(ctx: &Ctx<'_>, error: rquickjs::Error) -> anyhow::Error {
    if error.is_exception() {
        let exception = ctx.catch();
        if let Some(object) = exception.as_object() {
            let message = object.get::<_, String>("message").unwrap_or_default();
            let stack = object.get::<_, String>("stack").unwrap_or_default();
            return anyhow!("Svelte render failed: {message}\n{stack}");
        }
        return anyhow!("Svelte render failed: {exception:?}");
    }
    anyhow!("Svelte render failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_bundle(value: &str) -> Renderer {
        let mut files: BTreeMap<String, Arc<[u8]>> = UI_FILES
            .iter()
            .map(|(name, bytes)| ((*name).to_owned(), Arc::from(*bytes)))
            .collect();
        files.insert(
            "renderer.js".into(),
            Arc::from(format!("globalThis.aggrRender = () => {value:?}; void 0;").into_bytes()),
        );
        Renderer::from_files(files).unwrap()
    }

    #[test]
    fn ui_generations_swap_renderer_contexts_and_keep_in_flight_bytes() {
        let first = fake_bundle("first");
        let second = fake_bundle("second");
        assert_ne!(first.hash(), second.hash());
        assert_eq!(first.render_json("{}").unwrap(), "first");
        assert_eq!(second.render_json("{}").unwrap(), "second");
        assert_eq!(first.render_json("{}").unwrap(), "first");
    }

    #[test]
    fn incompatible_page_models_cannot_replace_the_current_reader() {
        let current = fake_bundle("current");
        let mut files = current.bundle.files.clone();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&files["assets.json"]).unwrap();
        manifest["contract"] = serde_json::json!("different-model");
        files.insert(
            "assets.json".into(),
            Arc::from(serde_json::to_vec(&manifest).unwrap()),
        );
        assert!(Renderer::from_files(files).is_err());
        assert_eq!(current.render_json("{}").unwrap(), "current");
    }

    #[test]
    fn invalid_disk_generation_does_not_replace_a_usable_bundle() {
        let root = tempfile::tempdir().unwrap();
        let first = fake_bundle("last good");
        let generation = root.path().join("a1");
        first.write_static(&generation).unwrap();
        for name in ["assets.json", "renderer.js"] {
            std::fs::write(generation.join(name), &first.bundle.files[name]).unwrap();
        }
        std::fs::write(root.path().join("current.json"), br#"{"directory":"a1"}"#).unwrap();
        let loaded = Renderer::from_current(root.path()).unwrap();
        assert_eq!(
            loaded.hash(),
            first.hash(),
            "disk and embedded generations use the same byte inventory"
        );
        std::fs::write(
            root.path().join("current.json"),
            br#"{"directory":"../outside"}"#,
        )
        .unwrap();
        assert!(Renderer::from_current(root.path()).is_err());
        std::fs::remove_file(generation.join("renderer.js")).unwrap();
        std::fs::write(root.path().join("current.json"), br#"{"directory":"a1"}"#).unwrap();
        assert!(Renderer::from_current(root.path()).is_err());
        assert_eq!(loaded.render_json("{}").unwrap(), "last good");
        let destination = root.path().join("published");
        loaded.write_static(&destination).unwrap();
        assert!(destination.join(&loaded.manifest.app).is_file());
    }

    #[test]
    fn embedded_renderer_has_no_host_io() {
        let renderer = Javascript::new().unwrap();
        renderer.context.with(|ctx| {
            let isolated: bool = ctx.eval("typeof process === 'undefined' && typeof fetch === 'undefined' && typeof document === 'undefined' && typeof require === 'undefined'").unwrap();
            assert!(isolated);
        });
    }

    #[test]
    fn manifest_contains_only_embedded_public_assets() {
        let renderer = Renderer::new().unwrap();
        assert_eq!(renderer.hash(), implementation_hash());
        for path in renderer.manifest.files.values() {
            assert!(path.starts_with("assets/"));
            assert!(!path.contains("renderer.js"));
            assert!(!embedded(path).unwrap().is_empty());
        }
        for path in renderer.eager_assets() {
            assert!(renderer.manifest.files.values().any(|value| value == &path));
        }
    }
}
