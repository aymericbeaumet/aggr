use std::time::{Duration, Instant};

use rquickjs::{Context, Runtime};

const TRANSITION: &str = include_str!("../vendor/packages/svelte/src/transition/index.js");
const MEDIA_QUERY: &str = include_str!("../vendor/packages/svelte/src/reactivity/media-query.js");
const ATTRIBUTES: &str =
    include_str!("../vendor/packages/svelte/src/internal/shared/attributes.js");
const DEBUG: &str = include_str!("../vendor/packages/svelte/src/internal/client/dev/debug.js");

fn function(source: &str, name: &str) -> String {
    let start = source.find(&format!("function {name}(")).unwrap();
    let end = source[start..].find("\n}").unwrap() + start + 2;
    source[start..end].to_owned()
}

fn evaluate(functions: &[(&str, &str)], test: &str) {
    let runtime = Runtime::new().unwrap();
    runtime.set_memory_limit(64 * 1024 * 1024);
    let deadline = Instant::now() + Duration::from_secs(5);
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        for (source, name) in functions {
            ctx.eval::<(), _>(function(source, name)).unwrap();
        }
        ctx.eval::<(), _>(
            r#"
            function equal(actual, expected) {
                if (JSON.stringify(actual) !== JSON.stringify(expected)) {
                    throw new Error(JSON.stringify({ actual, expected }).slice(0, 300));
                }
            }
            function combinations(alphabet, length, check) {
                function visit(value, left) {
                    check(value);
                    if (left) for (const character of alphabet) visit(value + character, left - 1);
                }
                visit('', length);
            }
        "#,
        )
        .unwrap();
        if ctx.eval::<(), _>(test).is_err() {
            panic!("runtime regression failed: {:?}", ctx.catch());
        }
    });
}

#[test]
fn css_units_preserve_parsing_without_overlapping_numeric_and_unit_scans() {
    evaluate(
        &[(TRANSITION, "split_css_unit")],
        r#"
        function previous(value) {
            const split = typeof value === 'string' && value.match(/^\s*(-?[\d.]+)([^\s]*)\s*$/);
            return split ? [parseFloat(split[1]), split[2] || 'px'] : [value, 'px'];
        }
        combinations(['0', '.', '-', 'x', ' '], 6, value => equal(split_css_unit(value), previous(value)));
        for (const value of [12, ' -1.5rem ', '\t2px\n', '2\npx', '.px', '-.px', '1..2em']) {
            equal(split_css_unit(value), previous(value));
        }
        equal(split_css_unit('.'.repeat(100000) + ' x'), ['.'.repeat(100000) + ' x', 'px']);
    "#,
    );
}

#[test]
fn media_query_parentheses_preserve_nonempty_same_line_matching() {
    evaluate(
        &[(MEDIA_QUERY, "has_parenthesized_content")],
        r#"
        combinations(['(', ')', 'x', '\n'], 7, value => {
            equal(has_parenthesized_content(value), /\(.+\)/.test(value));
        });
        for (const newline of ['\n', '\r', '\u2028', '\u2029']) {
            equal(has_parenthesized_content('(' + newline + ')'), false);
            equal(has_parenthesized_content('(' + newline + '(x)'), true);
        }
        equal(has_parenthesized_content('('.repeat(100000)), false);
        equal(has_parenthesized_content('('.repeat(100000) + ')'), true);
    "#,
    );
}

#[test]
fn css_comments_preserve_unterminated_and_multiline_input() {
    evaluate(
        &[
            (ATTRIBUTES, "strip_style_comments"),
            (ATTRIBUTES, "append_styles"),
            (ATTRIBUTES, "to_css_name"),
            (ATTRIBUTES, "to_style"),
        ],
        r#"
        combinations(['/', '*', 'x', '\n'], 7, value => {
            equal(strip_style_comments(value), value.replaceAll(/\/\*.*?\*\//g, ''));
        });
        for (const newline of ['\n', '\r', '\u2028', '\u2029']) {
            const value = 'a/* x' + newline + '/* y */z';
            equal(strip_style_comments(value), value.replaceAll(/\/\*.*?\*\//g, ''));
        }
        equal(strip_style_comments('/*' + 'a/*'.repeat(30000)), '/*' + 'a/*'.repeat(30000));
        equal(strip_style_comments('a/*'.repeat(30000) + '*/z'), 'az');
        equal(to_style('color: red; /* old */ width: 3px;', { color: 'blue' }), 'width: 3px; color: blue;');
        equal(to_style('content: "a; b"; /* keep declarations */ height: 2px;', [{ height: '4px' }, { color: 'red' }]),
            'content: "a; b"; height: 4px; color: red !important;');
    "#,
    );
}

#[test]
fn derived_debug_labels_preserve_nonempty_unwrapping_without_rescanning_suffixes() {
    evaluate(
        &[
            (DEBUG, "unwrap_derived_calls"),
            (DEBUG, "stringify_derived"),
        ],
        r#"
        function previous(source) {
            return '{' + source.replace(/^async \(\) => /, '').replace(/^\(/, '')
                .replace(/\$\.get\((.+?)\)/g, (_, name) => name)
                .replace(/\$\.track_reactivity_loss\((.+?)\)\(\)/g, (_, name) => name)
                .replace(/\)$/, '') + '}';
        }
        combinations(['$.get(', '$.track_reactivity_loss(', ')', ')()', 'x', '\n'], 5, value => {
            equal(stringify_derived({ toString: () => value }), previous(value));
        });
        for (const value of ['async () => ($.get(value))', '$.get()', '$.get($.get(value))',
            '$.track_reactivity_loss(a)b)()', '$.get(\u2028x)', '$.get(\rx)']) {
            equal(stringify_derived({ toString: () => value }), previous(value));
        }
        for (const prefix of ['$.get(', '$.track_reactivity_loss(']) {
            const value = prefix.repeat(20000);
            equal(stringify_derived({ toString: () => value }), '{' + value + '}');
        }
    "#,
    );
}
