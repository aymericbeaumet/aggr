use std::time::{Duration, Instant};

use rquickjs::{Context, Runtime};

fn run(script: &str) {
    let runtime = Runtime::new().unwrap();
    runtime.set_max_stack_size(8 * 1024 * 1024);
    let deadline = Instant::now() + Duration::from_secs(10);
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        ctx.eval::<(), _>("globalThis.performance = { now: () => Date.now() }; globalThis.console = { log() {}, warn() {}, error() {} }; globalThis.failure = null;").unwrap();
        ctx.eval::<(), _>(include_str!("../vendor/packages/svelte/compiler/index.js")).unwrap();
        if let Err(error) = ctx.eval::<(), _>(script) {
            panic!("tag regression: {error:?}: {:?}", ctx.catch());
        }
    });
    while runtime.is_job_pending() {
        runtime.execute_pending_job().unwrap();
    }
    context.with(|ctx| {
        let failure: Option<String> = ctx.globals().get("failure").unwrap();
        assert!(failure.is_none(), "{}", failure.unwrap_or_default());
    });
}

#[test]
fn tag_scanner_preserves_offsets_and_raw_text_boundaries() {
    let scanner = include_str!("../vendor/packages/svelte/src/compiler/utils/tag_scanner.js")
        .replacen("export function", "function", 1);
    run(&format!(
        r#"{scanner}
        function assert(value, message) {{ if (!value) throw new Error(message); }}
        const scan = create_tag_scanner(null, 'tsx');
        const input = '<div title="ordinary > attribute"></div><!--<style>fake</style>--><script a="1 > 0">const nested = "<style>fake</style>";</script><style>real</style>';
        const script = scan.exec(input);
        assert(script.tag === 'script' && script[3] === ' a="1 > 0"', 'quoted tag and attribute');
        assert(script[4] === 'const nested = "<style>fake</style>";', 'raw text excludes nested tags');
        assert(input.slice(script.index, scan.lastIndex) === script[0], 'script source offsets');
        const style = scan.exec(input);
        assert(style.tag === 'style' && style[4] === 'real', 'following style');
        assert(scan.exec(input) === null, 'no extra elements');
        for (const name of ['SCRIPT', 'STYLE', 'script.foo', 'style.foo', 'scripture', 'stylesheet']) {{
            const component = '<' + name + ' lang="ts">component</' + name + '>';
            for (const mode of ['lang', 'preprocess', 'migrate', 'tsx']) {{
                assert(create_tag_scanner(null, mode).exec(component) === null, 'component is not raw text: ' + name + ' ' + mode);
            }}
        }}
        const caseSensitive = create_tag_scanner('script', 'tsx').exec('<script>const text = "</SCRIPT>";</script>');
        assert(caseSensitive[4] === 'const text = "</SCRIPT>";', 'closing tags remain case-sensitive');
        const langs = create_tag_scanner('script', 'lang');
        assert(langs.exec('<script data-lang="ts">"<script lang=ts>"</script><script lang = "ts">')[2] === 'ts', 'actual lang attribute');
        assert(create_tag_scanner('script', 'lang').exec('<script data-lang="ts"></script>') === null, 'attribute name boundary');
        const expression = '<p title={{/"/.test("x") ? "yes" : "no"}}></p><script lang="ts">let value: number = 1;</script>';
        assert(create_tag_scanner('script', 'lang').exec(expression)[2] === 'ts', 'lang detection after Svelte expression');
        const moduleThenTyped = '<script module>export const x = 1;</script ><script lang="ts">let value: number = 1;</script>';
        assert(create_tag_scanner('script', 'lang').exec(moduleThenTyped)[2] === 'ts', 'compiler closing whitespace before typed instance script');
        const selfClosing = create_tag_scanner('script', 'preprocess').exec('<script src="external.js" />');
        assert(selfClosing[1] === ' src="external.js" ' && selfClosing[2] === undefined, 'self-closing preprocessing');
        for (const attack of ['<!--'.repeat(20000), '<script'.repeat(20000), '<script !=' + '\"\"\t!='.repeat(20000), '<script>' + '</script!'.repeat(20000)]) {{
            assert(create_tag_scanner(null, 'tsx').exec(attack) === null, 'unterminated input');
        }}
    "#
    ));
}

#[test]
fn svelte2tsx_artifacts_preserve_verbatim_nodes_without_backtracking() {
    for artifact in [
        include_str!("../vendor/packages/svelte2tsx/index.js"),
        include_str!("../vendor/packages/svelte2tsx/index.mjs"),
    ] {
        let start = artifact.find("function parseAttributes(").unwrap();
        let end = artifact.find("function blankVerbatimContent(").unwrap();
        let functions = &artifact[start..end];
        run(&format!(
            r#"{functions}
            function assert(value, message) {{ if (!value) throw new Error(message); }}
            const source = '<!--<script>ignored</script>--><script lang="ts" data-value="a > b">let value: number = 1;</script><style>p {{ color: red }}</style>';
            const nodes = findVerbatimElements(source);
            assert(nodes.length === 2 && nodes[0].type === 'Script' && nodes[1].type === 'Style', 'verbatim nodes');
            assert(nodes[0].attributes.lang === 'ts' && nodes[0].attributes['data-value'] === 'a > b', 'attributes');
            for (const node of nodes) {{
                assert(source.slice(node.content.start, node.content.end) === node.content.value, 'content source range');
                assert(source.slice(node.start, node.end).toLowerCase().startsWith('<' + node.name), 'container source range');
            }}
            for (const name of ['SCRIPT', 'STYLE', 'script.foo', 'style.foo']) {{
                const component = '<' + name + '>component</' + name + '>';
                const ordinary = findVerbatimElements(component + '<script>real</script><style>css</style>');
                assert(ordinary.length === 2 && ordinary[0].content.value === 'real' && ordinary[1].content.value === 'css', 'component tag boundary: ' + name);
                assert(ordinary[0].start === component.length, 'component preserved in source offsets');
            }}
            const expression = '<p title={{/"/.test("x") ? "yes" : "no"}}></p><script>let value = 1;</script>';
            const expressionNodes = findVerbatimElements(expression);
            assert(expressionNodes.length === 1 && expressionNodes[0].content.value === 'let value = 1;', 'Svelte expressions are not HTML attributes');
            const nested = findVerbatimElements('<script>const css = "<style>literal</style>";</script><style>actual</style>');
            assert(nested.length === 2 && nested[1].content.value === 'actual', 'nested raw text');
            assert(findVerbatimElements('<script !=' + '\"\"\t!='.repeat(20000)).length === 0, 'adversarial attributes');
            assert(findVerbatimElements('<!--'.repeat(20000)).length === 0, 'adversarial comments');
            assert(findVerbatimElements('<script>let x = 1;</script>'.repeat(10000)).length === 10000, 'many valid elements');
        "#
        ));
    }
}

#[test]
fn compiler_tag_scanning_handles_normal_and_adversarial_markup() {
    run(r#"
        function assert(value, message) { if (!value) throw new Error(message); }
        const typed = '<!-- <script lang="js"> -->\n<script data-note="a > b" lang="ts">let value: number = 1;</script><p>{value}</p>';
        assert(svelte.compile(typed, { generate: 'client' }).js.code.includes('value'), 'typed script with quoted angle');
        const moduleThenTyped = '<script module>export const x = 1;</script ><script lang="ts">let value: number = 1;</script><p>{value}</p>';
        assert(svelte.compile(moduleThenTyped, { generate: 'client' }).js.code.includes('value'), 'typed instance after module closing whitespace');
        const repeated = '<!--'.repeat(20000);
        try { svelte.parse(repeated); } catch (error) { assert(error.code === 'expected_token', error.message); }
        const malformed = '<script !=' + '\"\"\t!='.repeat(20000);
        try { svelte.parse(malformed); } catch (error) { assert(error.code !== undefined, error.message); }
        globalThis.failure = null;
        (async () => {
            for (const name of ['SCRIPT', 'STYLE', 'script.foo', 'style.foo']) {
                const component = '<' + name + '>component</' + name + '>';
                const visited = [];
                const result = await svelte.preprocess(component + '<script>real</script><style>css</style>', {
                    script: ({ content }) => { visited.push(content); return { code: 'changed' }; },
                    style: ({ content }) => { visited.push(content); return { code: 'changed-css' }; }
                });
                assert(result.code === component + '<script>changed</script><style>changed-css</style>', 'preprocess preserves component: ' + name);
                assert(JSON.stringify(visited) === '["real","css"]', 'only raw tags reach preprocess hooks');
                assert(svelte.compile(component, { generate: 'client' }).js.code.includes(name), 'compiler retains component identity');
            }
            const expression = '<p title={/"/.test("x") ? "yes" : "no"}></p>';
            const expressionSource = expression + '<script lang="ts">let value: number = 1;</script>';
            assert(svelte.compile(expressionSource, { generate: 'client' }).js.code.includes('value'), 'TS lang detection after Svelte expression');
            const expressionResult = await svelte.preprocess(expressionSource, { script: () => ({ code: 'let replaced = 1;' }) });
            assert(expressionResult.code === expression + '<script lang="ts">let replaced = 1;</script>', 'preprocess after Svelte expression');
            const seen = [];
            const result = await svelte.preprocess('<!--<script>hidden</script>--><script data-note="a > b">visible</script><style lang="css">p { color: red }</style>', {
                script: ({ content, attributes }) => { seen.push(content); assert(attributes['data-note'] === 'a > b', 'quoted attribute'); return { code: 'changed' }; },
                style: ({ content }) => ({ code: content.replace('red', 'blue') })
            });
            assert(JSON.stringify(seen) === '["visible"]', 'raw tag and comment scanning');
            assert(result.code.includes('changed') && result.code.includes('blue') && result.code.includes('hidden'), 'preprocess replacements');
            const attack = '<script !=' + '\"\"\t!='.repeat(20000);
            assert((await svelte.preprocess(attack, { script: () => { throw new Error('malformed script'); } })).code === attack, 'incomplete tag unchanged');
            const styles = '<style'.repeat(20000);
            assert((await svelte.preprocess(styles, { style: () => { throw new Error('malformed style'); } })).code === styles, 'repeated tag prefix unchanged');
            svelte.migrate(styles);
            const migrated = svelte.migrate('<script>let count = 0;</script><button on:click={() => count++}>{count}</button><style lang="scss">p { color: red; }</style>');
            assert(migrated.code.includes('$state(0)') && migrated.code.includes('p { color: red; }'), 'migration preserves style content');
        })().catch(error => { globalThis.failure = error.message; });
    "#);
}
