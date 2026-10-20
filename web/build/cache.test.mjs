import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { cachedUiBuild } from './cache.mjs';
import { uiFingerprint } from './inputs.mjs';
import { ensureDependencies } from './dependencies.mjs';
import { withLock } from './lock.mjs';

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'aggr-ui-build-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  for (const path of ['src', 'public', 'build', '.generated/types']) await mkdir(join(root, path), { recursive: true });
  await writeFile(join(root, 'package.json'), '{}');
  await writeFile(join(root, 'package-lock.json'), JSON.stringify({ packages: { 'node_modules/vite': { version: '1.0.0' } } }));
  await writeFile(join(root, 'src/app.ts'), 'export const value = 1;');
  return root;
}

async function artifact(directory) {
  await writeFile(join(directory, 'renderer.js'), 'renderer');
  await writeFile(join(directory, 'app.js'), 'app');
  await writeFile(join(directory, 'assets.json'), JSON.stringify({ app: 'app.js', files: { 'app.js': 'app.js' } }));
}

test('UI fingerprints ignore tests but cover production source, public files, generated types and build code', async t => {
  const root = await fixture(t);
  const original = await uiFingerprint(root);
  for (const path of ['src/app.test.ts', 'src/app.dom.test.ts', 'build/cache.test.mjs']) await writeFile(join(root, path), 'test only');
  assert.equal(await uiFingerprint(root), original);
  for (const path of ['src/app.ts', 'public/icon.svg', '.generated/types/Page.ts', 'build/compiler.mjs']) {
    const before = await uiFingerprint(root);
    await writeFile(join(root, path), `changed ${path}`);
    assert.notEqual(await uiFingerprint(root), before, path);
  }
});

test('one complete UI generation is reused across Cargo and frontend-test destinations', async t => {
  const root = await fixture(t);
  let builds = 0;
  const build = async path => { builds++; await artifact(path); };
  const first = await cachedUiBuild(root, join(root, 'cargo-ui'), build);
  const next = await cachedUiBuild(root, join(root, 'test-ui'), build);
  assert.equal(first.hit, false);
  assert.equal(next.hit, true);
  assert.equal(builds, 1);
  assert.equal(await readFile(join(root, 'test-ui/renderer.js'), 'utf8'), 'renderer');
  await writeFile(join(root, 'src/app.test.ts'), 'new test');
  await cachedUiBuild(root, join(root, 'test-ui'), build);
  assert.equal(builds, 1);
});

test('failed or incomplete builds never become cache hits or replace the prior destination', async t => {
  const root = await fixture(t);
  const destination = join(root, 'ui');
  await cachedUiBuild(root, destination, artifact);
  await writeFile(join(root, 'src/app.ts'), 'next');
  await assert.rejects(cachedUiBuild(root, destination, async path => { await writeFile(join(path, 'app.js'), 'partial'); }), /incomplete/i);
  assert.equal(await readFile(join(destination, 'renderer.js'), 'utf8'), 'renderer');
  const recovered = await cachedUiBuild(root, destination, artifact);
  assert.equal(recovered.hit, false);
});

test('concurrent consumers share one build and corrupted output is repaired', async t => {
  const root = await fixture(t);
  let builds = 0;
  const build = async path => { builds++; await new Promise(resolve => setTimeout(resolve, 40)); await artifact(path); };
  const [first, second] = await Promise.all([cachedUiBuild(root, join(root, 'a'), build), cachedUiBuild(root, join(root, 'b'), build)]);
  assert.equal(first.key, second.key);
  assert.equal(builds, 1);
  await writeFile(join(root, '.generated/ui-cache', first.key, 'artifact/app.js'), 'corrupt');
  await cachedUiBuild(root, join(root, 'a'), build);
  assert.equal(builds, 2);
  assert.equal(await readFile(join(root, 'a/app.js'), 'utf8'), 'app');
});

test('dependency bootstrap repairs missing packages even when the installation receipt exists', async t => {
  const root = await fixture(t);
  let installs = 0;
  const install = async () => {
    installs++;
    await mkdir(join(root, 'node_modules/vite'), { recursive: true });
    await writeFile(join(root, 'node_modules/vite/package.json'), '{"version":"1.0.0"}');
  };
  assert.equal(await ensureDependencies(root, install), true);
  assert.equal(await ensureDependencies(root, install), false);
  await rm(join(root, 'node_modules/vite'), { recursive: true });
  await Promise.all([ensureDependencies(root, install), ensureDependencies(root, install)]);
  assert.equal(installs, 2);
});

test('an edit arriving during compilation cannot poison an older cache key', async t => {
  const root = await fixture(t);
  const old = await uiFingerprint(root);
  let builds = 0;
  const result = await cachedUiBuild(root, join(root, 'ui'), async path => {
    builds++;
    await artifact(path);
    if (builds === 1) await writeFile(join(root, 'src/app.ts'), 'changed during build');
  });
  assert.equal(builds, 2);
  assert.notEqual(result.key, old);
  await assert.rejects(readFile(join(root, '.generated/ui-cache', old, 'complete.json')), { code: 'ENOENT' });
});

test('interrupted lock owners are recovered without treating a live build as abandoned', async t => {
  const root = await fixture(t);
  const lock = join(root, '.generated/interrupted.lock');
  await mkdir(lock);
  await writeFile(join(lock, 'owner.json'), JSON.stringify({ pid: 99_999_999, token: 'interrupted' }));
  let active = 0;
  let maximum = 0;
  const run = () => withLock(lock, async () => {
    active++;
    maximum = Math.max(maximum, active);
    await new Promise(resolve => setTimeout(resolve, 30));
    active--;
  });
  await Promise.all([run(), run(), run()]);
  assert.equal(maximum, 1);
});
