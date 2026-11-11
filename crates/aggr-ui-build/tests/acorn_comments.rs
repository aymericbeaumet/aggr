use std::time::{Duration, Instant};

use rquickjs::{Context, Module, Runtime};

fn with_acorn(module: bool, test: &str) {
    let runtime = Runtime::new().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        if module {
            Module::declare(
                ctx.clone(),
                "acorn",
                include_str!("../vendor/packages/acorn/dist/acorn.mjs"),
            )
            .unwrap();
            Module::evaluate(
                ctx.clone(),
                "test",
                format!("import * as acorn from 'acorn';\n{test}"),
            )
            .unwrap()
            .finish::<()>()
            .unwrap_or_else(|error| panic!("{error}: {:?}", ctx.catch()));
        } else {
            ctx.eval::<(), _>(include_str!("../vendor/packages/acorn/dist/acorn.js"))
                .unwrap();
            ctx.eval::<(), _>(test)
                .unwrap_or_else(|error| panic!("{error}: {:?}", ctx.catch()));
        }
    });
}

#[test]
fn comments_preserve_directives_and_contextual_keyword_lookahead() {
    for module in [false, true] {
        with_acorn(
            module,
            r#"
            const parse = source => acorn.parse(source, {ecmaVersion: 'latest'});
            function assert(value) { if (!value) throw new Error('parser contract'); }
            function rejects(source, message) {
                try { parse(source); } catch (error) {
                    assert(error.message.includes(message)); return;
                }
                throw new Error('expected syntax error');
            }
            for (const whitespace of [' ', '\t', '\v', '\f', '\r\n', '\u00a0', '\u1680', '\u2000', '\u2028', '\u2029', '\u202f', '\u205f', '\u3000', '\ufeff']) {
                rejects(whitespace + '/* before */ "use strict" /* after */; with ({}) {}', 'strict');
                assert(parse('let' + whitespace + '/* binding */ [x] = [1]').body[0].kind === 'let');
            }
            rejects('"other"; // heading\n "use strict"\n with ({}) {}', 'strict');
            rejects('"use strict"/*\n*/with ({}) {}', 'strict');
            assert(parse('"use strict"/*\n*/[0]; with ({}) {}').body.length === 2);
            assert(parse('"use strict" + "suffix"; with ({}) {}').body.length === 2);
            assert(parse('async /* same line */ function f() {}').body[0].async);
            assert(parse('async /*\n*/ function f() {}').body.length === 2);
            for (const newline of ['\r', '\n', '\u2028', '\u2029']) {
                assert(parse('async // comment' + newline + 'function f() {}').body.length === 2);
            }
            assert(parse('{ using /* resource */ item = acquire(); }').body[0].body[0].kind === 'using');
            rejects('"use strict"; /* unterminated', 'Unterminated comment');
            "#,
        );
    }
}

#[test]
fn adversarial_comment_prefixes_finish_with_the_correct_parse_result() {
    for module in [false, true] {
        with_acorn(
            module,
            r#"
            const parse = source => acorn.parse(source, {ecmaVersion: 'latest'});
            // CodeQL's repeated comment-opener input must remain linear, including
            // the unterminated case encountered by directive and keyword lookahead.
            const comment = '/*' + '\t/*'.repeat(60000);
            for (const prefix of ['', '"use strict";', 'let ', 'async ']) {
                let rejected = false;
                try { parse(prefix + comment); } catch (error) {
                    if (!error.message.includes('Unterminated comment')) throw error;
                    rejected = true;
                }
                if (!rejected) throw new Error('unterminated comment accepted');
            }
            const program = parse(comment + '*/ "use strict"; const answer = 42;');
            if (program.body.length !== 2 || program.body[0].directive !== 'use strict') {
                throw new Error('closed comment changed directive parsing');
            }
            "#,
        );
    }
}

#[test]
fn embedded_svelte_parser_handles_adversarial_javascript_and_typescript_comments() {
    let runtime = Runtime::new().unwrap();
    runtime.set_memory_limit(512 * 1024 * 1024);
    runtime.set_max_stack_size(8 * 1024 * 1024);
    let deadline = Instant::now() + Duration::from_secs(10);
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        ctx.eval::<(), _>("globalThis.performance = {now: () => Date.now()};")
            .unwrap();
        ctx.eval::<(), _>(include_str!("../vendor/packages/svelte/compiler/index.js"))
            .unwrap_or_else(|error| panic!("{error}: {:?}", ctx.catch()));
        ctx.eval::<(), _>(
            r#"
            const comment = '/*' + '\t/*'.repeat(60000);
            for (const typescript of [false, true]) {
                const tag = typescript ? '<script lang="ts">' : '<script>';
                const declaration = typescript ? 'value: number = 42;' : 'value = 42;';
                const result = svelte.compile(tag + 'let ' + comment + '*/ ' + declaration + '</script><p>{value}</p>',
                    {filename: 'Comments.svelte', generate: 'client'});
                if (!result.js.code.includes('42')) throw new Error('compiler lost declaration');
                let rejected = false;
                try {
                    svelte.compile(tag + 'let ' + comment + '</script>', {filename: 'Broken.svelte'});
                } catch (error) {
                    if (!error.message.includes('Unterminated comment')) throw error;
                    rejected = true;
                }
                if (!rejected) throw new Error('unterminated comment accepted');
            }
            "#,
        )
        .unwrap_or_else(|error| panic!("{error}: {:?}", ctx.catch()));
    });
}
