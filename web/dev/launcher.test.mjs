import assert from 'node:assert/strict';
import { join, resolve } from 'node:path';
import test from 'node:test';
import { buildArguments, needsBuild, readContext, workerArguments } from './context.mjs';

const context = {
  repository: resolve('source checkout with spaces'),
  cwd: resolve('reader with spaces'),
  executable: resolve('target/debug/aggr'),
  workerArgs: ['--config', 'reader; $(touch never).toml', 'dev', '--port', '8123', '--clean', '--base-url', 'https://example.com/?literal=$(touch never)&x=1'],
  viteOrigin: 'http://127.0.0.1:8123',
};

test('launcher preserves argument boundaries while keeping the public port out of the worker', () => {
  const decoded = readContext(JSON.stringify(context));
  assert.deepEqual(decoded, context);
  const arguments_ = workerArguments(decoded, false);
  assert.equal(arguments_[1], context.workerArgs[1]);
  assert.equal(arguments_[arguments_.indexOf('--base-url') + 1], context.workerArgs.at(-1));
  assert.deepEqual(arguments_.slice(-2), ['--port', '0']);
  assert(!arguments_.includes('8123'));
  assert(arguments_.includes('--clean'));
  assert(!workerArguments(decoded, true).includes('--clean'));
});

test('rebuilds target a different executable and exclude frontend edits in ordinary dev', () => {
  const arguments_ = buildArguments(context);
  assert.equal(arguments_[0], 'build');
  assert.equal(arguments_[arguments_.indexOf('--bin') + 1], 'aggr-dev-worker');
  assert.equal(arguments_[arguments_.indexOf('--manifest-path') + 1], join(context.repository, 'Cargo.toml'));
  assert(!arguments_.includes('run'));
  for (const file of ['src/site/client.rs', 'crates/aggr-ui-model/src/lib.rs', 'crates/aggr-ui-model/Cargo.toml', 'Cargo.lock', 'build.rs']) {
    assert(needsBuild(context, join(context.repository, file)), file);
  }
  for (const file of ['config.default.toml', 'examples/starter.toml', 'crates/aggr-lib/src/store/branch-readme.md', 'crates/aggr-lib/src/media/fonts/AtkinsonHyperlegible-Regular.ttf', 'web/public/assets/favicon-32.png']) {
    assert(needsBuild(context, join(context.repository, file)), file);
  }
  for (const file of ['web/src/App.svelte', 'web/.generated/types/ClientPage.ts', 'target/debug/aggr', 'aggr.toml']) {
    assert(!needsBuild(context, join(context.repository, file)), file);
  }
  const release = { ...context, workerArgs: [...context.workerArgs, '--release'] };
  assert(needsBuild(release, join(context.repository, 'web/src/App.svelte')));
  assert(!needsBuild(release, join(context.repository, 'web/node_modules/vite/index.js')));
});

test('missing or malformed contexts fail before a subprocess starts', () => {
  assert.throws(() => readContext(''), /cargo run -- dev or make dev/);
  assert.throws(() => readContext('{'), SyntaxError);
  for (const invalid of [{}, { ...context, cwd: 'relative' }, { ...context, executable: '' }, { ...context, workerArgs: [1] }]) {
    assert.throws(() => readContext(JSON.stringify(invalid)), /Invalid development launcher context/);
  }
});
