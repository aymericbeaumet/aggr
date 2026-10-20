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

include!(concat!(env!("OUT_DIR"), "/ui.rs"));

pub fn implementation_hash() -> &'static str {
    env!("AGGR_UI_HASH")
}

fn embedded(name: &str) -> Result<&'static [u8]> {
    UI_FILES
        .iter()
        .find(|(path, _)| *path == name)
        .map(|(_, bytes)| *bytes)
        .with_context(|| format!("missing embedded UI asset {name}"))
}

#[derive(Debug, Deserialize)]
struct Manifest {
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
}

impl Renderer {
    pub fn new() -> Result<Self> {
        let manifest: Manifest = serde_json::from_slice(embedded("assets.json")?)
            .context("reading embedded UI manifest")?;
        for path in manifest.files.values() {
            anyhow::ensure!(
                path.starts_with("assets/") && !path.split('/').any(|part| part == ".."),
                "invalid UI asset path {path}"
            );
            embedded(path)?;
        }
        Ok(Self {
            manifest: Arc::new(manifest),
        })
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
        RENDERER
            .with(|slot| {
                let mut slot = slot.borrow_mut();
                if slot.is_none() {
                    *slot = Some(Javascript::new()?);
                }
                let result = slot
                    .as_ref()
                    .context("initializing reader renderer")?
                    .render(&json);
                if result.is_err() {
                    // An interrupted or exhausted context cannot affect the following page.
                    *slot = None;
                }
                result
            })
            .with_context(|| format!("rendering {}", request.page.path))
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
            std::fs::write(&destination, embedded(path)?)
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
    static RENDERER: RefCell<Option<Javascript>> = const { RefCell::new(None) };
}

struct Javascript {
    context: Context,
    deadline: Rc<Cell<Option<Instant>>>,
    _runtime: Runtime,
}

impl Javascript {
    fn new() -> Result<Self> {
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
        let source = embedded("renderer.js")?;
        context.with(|ctx| {
            ctx.eval::<(), _>(source)
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
