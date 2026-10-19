import assert from 'node:assert/strict';
import { join, resolve } from 'node:path';
import test from 'node:test';
import { backendArguments, commands, readContext } from './context.mjs';

const context = {
  repository: resolve('source checkout with spaces'),
  cwd: resolve('reader with spaces'),
  workerArgs: ['--config', 'reader; $(touch never).toml', 'dev', '--port', '8123', '--base-url', 'https://example.com/?literal=$(touch never)&x=1'],
  viteOrigin: 'http://127.0.0.1:5173',
};

test('launcher transports config and flags as JSON without putting them in shell commands', () => {
  const decoded = readContext(JSON.stringify(context));
  assert.deepEqual(decoded, context);
  for (const command of commands(decoded)) {
    assert(!command.command.includes('touch'));
    assert(!command.command.includes(context.cwd));
    assert.deepEqual(JSON.parse(command.env.AGGR_DEV_CONTEXT).workerArgs, context.workerArgs);
  }
});

test('backend preserves worker argument boundaries and watches only Rust and Cargo inputs', () => {
  const arguments_ = backendArguments(context);
  const cargo = arguments_.indexOf('cargo');
  assert.deepEqual(arguments_.slice(cargo), ['cargo', 'run', '--locked', '--manifest-path', join(context.repository, 'Cargo.toml'), '--', ...context.workerArgs]);
  assert(arguments_.includes('--shell=none'));
  assert(arguments_.includes(join(context.repository, 'src')));
  assert(arguments_.includes('/src/theme/**'));
  assert(!arguments_.includes('web/src/generated'));
  assert.equal(arguments_[arguments_.indexOf('--stop-signal') + 1], 'SIGINT');
});

test('missing or malformed contexts fail before a subprocess starts', () => {
  assert.throws(() => readContext(''), /cargo run -- dev or make dev/);
  assert.throws(() => readContext('{'), SyntaxError);
  for (const invalid of [{}, { ...context, cwd: 'relative' }, { ...context, workerArgs: [1] }]) {
    assert.throws(() => readContext(JSON.stringify(invalid)), /Invalid development launcher context/);
  }
});
