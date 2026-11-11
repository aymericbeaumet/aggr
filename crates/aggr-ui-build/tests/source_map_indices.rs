use rquickjs::{Context, Module, Runtime};

fn evaluate(module: bool, test: &str) {
    let runtime = Runtime::new().unwrap();
    let context = Context::full(&runtime).unwrap();
    context.with(|ctx| {
        for name in [
            "sourcemap-codec",
            "resolve-uri",
            "trace-mapping",
            "gen-mapping",
        ] {
            let extension = if module { "mjs" } else { "umd.js" };
            let path = aggr_ui_build::vendor_root().join(format!(
                "packages/@jridgewell/{name}/dist/{name}.{extension}"
            ));
            let source = std::fs::read_to_string(path).unwrap();
            if module {
                Module::declare(ctx.clone(), format!("@jridgewell/{name}"), source).unwrap();
            } else {
                ctx.eval::<(), _>(source).unwrap();
            }
        }
        if module {
            Module::evaluate(
                ctx.clone(),
                "test",
                format!("import * as genMapping from '@jridgewell/gen-mapping';\n{test}"),
            )
            .unwrap()
            .finish::<()>()
            .unwrap_or_else(|error| panic!("{error}: {:?}", ctx.catch()));
        } else {
            ctx.eval::<(), _>(test)
                .unwrap_or_else(|error| panic!("{error}: {:?}", ctx.catch()));
        }
    });
}

#[test]
fn source_map_lines_reject_property_names_without_mutating_prototypes() {
    for module in [false, true] {
        evaluate(
            module,
            r#"
            for (const line of ['__proto__', 'constructor', -1, 0.5, NaN, Infinity, 4294967295, {valueOf: () => 0}]) {
                const map = new genMapping.GenMapping();
                let rejected = false;
                try { genMapping.addSegment(map, line, 0); }
                catch (error) { rejected = error instanceof RangeError; }
                if (!rejected) throw new Error('invalid line accepted: ' + line);
                if (Object.hasOwn(Array.prototype, '0')) throw new Error('array prototype changed');
                if (genMapping.toDecodedMap(map).mappings.length !== 0) throw new Error('map changed');
            }
        "#,
        );
    }
}

#[test]
fn source_map_insertion_keeps_columns_sorted_and_duplicate_order() {
    for module in [false, true] {
        evaluate(
            module,
            r#"
            const map = new genMapping.GenMapping();
            genMapping.addSegment(map, 0, 20, 'first.ts', 0, 2);
            genMapping.addSegment(map, 0, 0, 'first.ts', 0, 0);
            genMapping.addSegment(map, 0, 10, 'first.ts', 0, 1);
            genMapping.addSegment(map, 0, 10, 'second.ts', 1, 3);
            genMapping.addSegment(map, 2, 1);
            const result = genMapping.toDecodedMap(map);
            const expected = [[[0,0,0,0],[10,0,0,1],[10,1,1,3],[20,0,0,2]],[],[ [1] ]];
            if (JSON.stringify(result.mappings) !== JSON.stringify(expected)) throw new Error('mapping order changed');
            const roundtrip = genMapping.toDecodedMap(genMapping.fromMap(genMapping.toEncodedMap(map)));
            if (JSON.stringify(roundtrip.mappings) !== JSON.stringify(expected)) throw new Error('roundtrip changed');
        "#,
        );
    }
}
