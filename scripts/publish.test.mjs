import assert from 'node:assert/strict';
import test from 'node:test';
import { publishPackages } from './publish.mjs';

const metadata = { packages: [
  { name: 'aggr', version: '2.0.0' },
  { name: 'aggr-lib', version: '2.0.0' },
  { name: 'aggr-ui-model', version: '0.1.0' },
] };

function fixture(published = {}) {
  const calls = [], logs = [], requests = [];
  const options = {
    metadata, version: '2.0.0', env: { GITHUB_ACTIONS: 'true', CARGO_REGISTRY_TOKEN: 'test-only-token' },
    log: message => logs.push(message),
    fetchIndex: async (url, init) => {
      requests.push({ url, init });
      const name = new URL(url).pathname.split('/').at(-1);
      if (!Object.hasOwn(published, name)) return new Response('', { status: 404 });
      return new Response(published[name].map(entry => JSON.stringify({
        name, yanked: false, ...(typeof entry === 'string' ? { vers: entry } : entry),
      })).join('\n'));
    },
    run: async (command, args, options) => calls.push({ command, args, ...options }),
  };
  return { calls, logs, requests, options, publish: overrides => publishPackages({ ...options, ...overrides }) };
}

test('first publication verifies packages and publishes the model, library and CLI in order', async () => {
  const f = fixture();
  await f.publish({ args: ['--allow-dirty'], ref: { type: 'tag', name: 'v2.0.0' } });
  assert.deepEqual(f.calls.map(({ command, args }) => [command, args]), [
    ['make', ['package', 'PACKAGE_ARGS=--allow-dirty']],
    ['cargo', ['publish', '--registry', 'crates-io', '-p', 'aggr-ui-model@0.1.0', '-p', 'aggr-lib@2.0.0', '-p', 'aggr@2.0.0', '--allow-dirty']],
  ]);
  assert.equal(f.calls[0].env.CARGO_REGISTRY_TOKEN, undefined);
  assert.equal(f.calls[1].env.CARGO_REGISTRY_TOKEN, 'test-only-token');
  assert.ok(f.requests.every(({ init }) => !new Headers(init.headers).has('authorization')));
});

test('a subsequent release publishes the library and CLI while skipping the existing model', async () => {
  const f = fixture({ 'aggr-ui-model': ['0.1.0'] });
  await f.publish();
  assert.deepEqual(f.calls[1].args, ['publish', '--registry', 'crates-io', '-p', 'aggr-lib@2.0.0', '-p', 'aggr@2.0.0']);
  assert.ok(f.logs.some(message => message.includes('aggr-ui-model 0.1.0 already published')));
});

test('retrying a completed publication does not build, package or upload again', async () => {
  const f = fixture({ 'aggr-ui-model': ['0.1.0'], 'aggr-lib': ['2.0.0'], aggr: ['2.0.0'] });
  await f.publish();
  assert.deepEqual(f.calls, []);
  assert.ok(f.logs.some(message => message.includes('Nothing to publish')));
});

test('an already published library is skipped while a missing model version remains publishable', async () => {
  const f = fixture({ 'aggr-lib': ['2.0.0'], aggr: ['2.0.0'] });
  await f.publish();
  assert.deepEqual(f.calls[1].args, ['publish', '--registry', 'crates-io', '-p', 'aggr-ui-model@0.1.0']);
});

test('retrying after both prerequisites were published uploads only the missing CLI', async () => {
  const f = fixture({ 'aggr-ui-model': ['0.1.0'], 'aggr-lib': ['2.0.0'] });
  await f.publish();
  assert.deepEqual(f.calls[1].args, ['publish', '--registry', 'crates-io', '-p', 'aggr@2.0.0']);
});

test('a yanked prerequisite is not treated as a usable published dependency', async () => {
  const f = fixture({ 'aggr-ui-model': [{ vers: '0.1.0', yanked: true }] });
  await assert.rejects(f.publish(), /aggr-ui-model 0.1.0 is yanked/);
  assert.deepEqual(f.calls, []);
});

test('registry failures or malformed responses never become permission to publish', async () => {
  for (const response of [new Response('unavailable', { status: 503 }), new Response('invalid'), new Response('')]) {
    const f = fixture();
    await assert.rejects(f.publish({ fetchIndex: async () => response.clone() }));
    assert.deepEqual(f.calls, []);
  }
});

test('package verification failure stops before any upload', async () => {
  const f = fixture();
  await assert.rejects(f.publish({ run: async command => { assert.equal(command, 'make'); throw new Error('package failed'); } }), /package failed/);
});

test('version, tag and argument mistakes fail before registry access', async () => {
  for (const overrides of [
    { version: '2.0.1' }, { ref: { type: 'tag', name: 'v2.0.1' } },
    { args: ['--registry', 'elsewhere'] },
  ]) {
    const f = fixture();
    await assert.rejects(f.publish(overrides));
    assert.deepEqual(f.requests, []);
    assert.deepEqual(f.calls, []);
  }
});

test('CI needs a token only when an upload is pending', async () => {
  const missing = fixture();
  await assert.rejects(missing.publish({ env: { GITHUB_ACTIONS: 'true' } }), /CARGO_REGISTRY_TOKEN/);
  assert.equal(missing.requests.length, 3);
  assert.deepEqual(missing.calls, []);
  const complete = fixture({ 'aggr-ui-model': ['0.1.0'], 'aggr-lib': ['2.0.0'], aggr: ['2.0.0'] });
  await complete.publish({ env: { GITHUB_ACTIONS: 'true' } });
  assert.deepEqual(complete.calls, []);
});
