use std::{
    path::Path,
    sync::{OnceLock, mpsc},
    thread,
};

use anyhow::{Context as _, Result, anyhow, ensure};
use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{TransformOptions, Transformer};
use rquickjs::{Context, Function, Runtime};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Official Svelte compiler output, with warnings retained for validation tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compiled {
    pub code: String,
    pub css: Option<String>,
    pub warnings: serde_json::Value,
}

/// Strip TypeScript before compiling runes in a standalone `.svelte.ts` module.
pub fn strip_typescript(source: &str, filename: &str) -> Result<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    ensure!(
        parsed.diagnostics.is_empty(),
        "{filename}: TypeScript parse errors: {:?}",
        parsed.diagnostics
    );
    let mut program = parsed.program;
    let semantic = SemanticBuilder::new()
        .with_excess_capacity(2.0)
        .with_enum_eval(true)
        .build(&program);
    ensure!(
        semantic.diagnostics.is_empty(),
        "{filename}: TypeScript analysis errors: {:?}",
        semantic.diagnostics
    );
    let transformed = Transformer::new(
        &allocator,
        Path::new(filename),
        &TransformOptions::default(),
    )
    .build_with_scoping(semantic.semantic.into_scoping(), &mut program);
    ensure!(
        transformed.diagnostics.is_empty(),
        "{filename}: TypeScript transform errors: {:?}",
        transformed.diagnostics
    );
    Ok(Codegen::new().build(&program).code)
}

struct Request {
    source: String,
    filename: String,
    server: bool,
    module: bool,
    reply: mpsc::Sender<std::result::Result<Compiled, String>>,
}

fn compiler() -> Result<&'static mpsc::Sender<Request>> {
    static COMPILER: OnceLock<std::result::Result<mpsc::Sender<Request>, String>> = OnceLock::new();
    let compiler = COMPILER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Request>();
        thread::Builder::new()
            .name("svelte-compiler".into())
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                let engine = Compiler::new().map_err(|error| format!("{error:#}"));
                for request in receiver {
                    let result = match &engine {
                        Ok(engine) => engine
                            .compile(&request)
                            .map_err(|error| format!("{error:#}")),
                        Err(error) => Err(error.clone()),
                    };
                    let _ = request.reply.send(result);
                }
            })
            .map_err(|error| format!("starting official Svelte compiler: {error}"))?;
        Ok(sender)
    });
    compiler.as_ref().map_err(|error| anyhow!(error.clone()))
}

pub(crate) fn fingerprint() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        let mut digest = Sha256::new();
        digest.update(env!("CARGO_PKG_VERSION").as_bytes());
        digest.update(include_bytes!("compiler.rs"));
        digest.update(include_bytes!(
            "../vendor/packages/svelte/compiler/index.js"
        ));
        hex::encode(digest.finalize())
    })
}

struct Compiler {
    context: Context,
    _runtime: Runtime,
}

impl Compiler {
    fn new() -> Result<Self> {
        let runtime = Runtime::new().context("creating Svelte compiler runtime")?;
        runtime.set_memory_limit(512 * 1024 * 1024);
        runtime.set_max_stack_size(8 * 1024 * 1024);
        let context = Context::full(&runtime)?;
        context.with(|ctx| -> Result<()> {
            ctx.eval::<(), _>("globalThis.performance={now:()=>Date.now()};")?;
            ctx.eval::<(), _>(include_str!("../vendor/packages/svelte/compiler/index.js"))
                .map_err(|error| js_error(&ctx, error))?;
            ctx.eval::<(), _>(r#"globalThis.aggrCompile = (source, filename, server, module) => {
                try {
                    const options = {filename, generate: server ? 'server' : 'client', dev:false};
                    const result = module ? svelte.compileModule(source, options) :
                        svelte.compile(source, {...options, runes:true, css:'external', discloseVersion:false});
                    return JSON.stringify({code:result.js.code,css:result.css?.code ?? null,
                        warnings:result.warnings.map(({code,message,start,end,frame})=>({code,message,start,end,frame}))});
                } catch(error) {
                    throw new Error(JSON.stringify({filename,code:error.code,message:error.message,
                        start:error.start,end:error.end,frame:error.frame}));
                }
            };"#).map_err(|error| js_error(&ctx, error))?;
            Ok(())
        })?;
        Ok(Self {
            context,
            _runtime: runtime,
        })
    }

    fn compile(&self, request: &Request) -> Result<Compiled> {
        self.context.with(|ctx| {
            let function: Function = ctx.globals().get("aggrCompile")?;
            let json: String = function
                .call((
                    request.source.as_str(),
                    request.filename.as_str(),
                    request.server,
                    request.module,
                ))
                .map_err(|error| js_error(&ctx, error))?;
            serde_json::from_str(&json).context("reading Svelte compiler output")
        })
    }
}

fn js_error(ctx: &rquickjs::Ctx<'_>, error: rquickjs::Error) -> anyhow::Error {
    if error.is_exception() {
        let exception = ctx.catch();
        if let Some(object) = exception.as_object()
            && let Ok(message) = object.get::<_, String>("message")
        {
            return anyhow!(message);
        }
        anyhow!("Svelte compiler exception: {exception:?}")
    } else {
        error.into()
    }
}

fn compile(source: &str, filename: &str, server: bool, module: bool) -> Result<Compiled> {
    let (reply, receiver) = mpsc::channel();
    compiler()?
        .send(Request {
            source: source.into(),
            filename: filename.into(),
            server,
            module,
            reply,
        })
        .context("sending Svelte compiler request")?;
    receiver
        .recv()
        .context("Svelte compiler stopped")?
        .map_err(anyhow::Error::msg)
}

/// Compile one component with the official Svelte 5.57.2 compiler.
pub fn compile_component(source: &str, filename: &str, server: bool) -> Result<Compiled> {
    compile(source, filename, server, false)
}

/// Compile reactive TypeScript/JavaScript modules for the selected runtime.
pub fn compile_module(source: &str, filename: &str, server: bool) -> Result<Compiled> {
    let javascript = if filename.ends_with(".ts") {
        strip_typescript(source, filename)?
    } else {
        source.into()
    };
    compile(&javascript, filename, server, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_compiler_emits_server_and_hydratable_browser_code() {
        let source = r#"<script lang="ts">let {name}: {name: string} = $props();</script><h1>{name}</h1><style>h1{color:red}</style>"#;
        let browser = compile_component(source, "Greeting.svelte", false).unwrap();
        let server = compile_component(source, "Greeting.svelte", true).unwrap();
        assert!(browser.code.contains("svelte/internal/client"));
        assert!(server.code.contains("svelte/internal/server"));
        assert!(browser.css.as_ref().unwrap().contains("color:red"));
        assert!(!server.code.contains("name: string"));
    }

    #[test]
    fn typed_class_state_modules_transform_before_rune_compilation() {
        let source = "export class State { value = $state.raw<string | null>(null); }";
        let browser = compile_module(source, "state.svelte.ts", false).unwrap();
        let server = compile_module(source, "state.svelte.ts", true).unwrap();
        assert!(!browser.code.contains("$state"));
        assert!(!server.code.contains("$state"));
        assert!(browser.code.contains("svelte/internal/client"));
    }

    #[test]
    fn syntax_errors_retain_source_location_and_do_not_poison_next_compile() {
        let error = compile_component("<div>{#if}", "Broken.svelte", false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Broken.svelte"));
        assert!(error.contains("start"));
        assert!(compile_component("<p>Next</p>", "Next.svelte", false).is_ok());
    }
}
