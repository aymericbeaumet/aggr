import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const entry = fileURLToPath(new URL('./start.mjs', import.meta.url));
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));

async function waitFor(check, label) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    if (await check()) return;
    await delay(50);
  }
  throw new Error(`${label} timed out`);
}

async function fixture(t) {
  const repository = await mkdtemp(join(tmpdir(), 'aggr-dev-lifecycle-'));
  await mkdir(join(repository, 'web/dev'), { recursive: true });
  const ready = join(repository, 'ready.json');
  const stopped = join(repository, 'stopped');
  await writeFile(join(repository, 'web/dev/backend.mjs'), `
import { watch, writeFileSync, existsSync } from 'node:fs';
const context = JSON.parse(process.env.AGGR_DEV_CONTEXT);
writeFileSync(process.env.AGGR_TEST_READY, JSON.stringify(context));
process.on('SIGINT', () => { writeFileSync(process.env.AGGR_TEST_STOPPED, 'stopped'); process.exit(0); });
watch(context.cwd, () => { if (existsSync(context.cwd + '/crash')) process.exit(7); });
`);
  const occupied = createServer();
  await new Promise(resolve => occupied.listen(0, '127.0.0.1', resolve));
  t.after(async () => {
    await new Promise(resolve => occupied.close(resolve));
    await rm(repository, { recursive: true, force: true });
  });
  return { repository, ready, stopped, port: occupied.address().port };
}

function start(t, state, strictPort) {
  const context = { repository: state.repository, cwd: state.repository, workerArgs: ['dev'], viteOrigin: `http://127.0.0.1:${state.port}`, strictPort };
  const child = spawn(process.execPath, [entry], {
    env: { ...process.env, AGGR_DEV_CONTEXT: JSON.stringify(context), AGGR_TEST_READY: state.ready, AGGR_TEST_STOPPED: state.stopped },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '';
  child.stdout.on('data', chunk => { output += chunk; });
  child.stderr.on('data', chunk => { output += chunk; });
  t.after(async () => {
    if (child.exitCode === null && child.signalCode === null) {
      child.kill('SIGINT');
      await waitFor(() => child.exitCode !== null || child.signalCode !== null, 'test process cleanup');
    }
  });
  return { child, output: () => output };
}

async function backendReady(state, running) {
  let context;
  await waitFor(async () => {
    assert.equal(running.child.exitCode, null, running.output());
    try { context = JSON.parse(await readFile(state.ready, 'utf8')); return true; }
    catch (error) { if (error.code === 'ENOENT') return false; throw error; }
  }, 'backend readiness');
  return context;
}

test('an occupied default port selects a free Vite origin and SIGINT cleans up the stack', { skip: process.platform === 'win32' }, async t => {
  const state = await fixture(t);
  const running = start(t, state, false);
  const context = await backendReady(state, running);
  assert.notEqual(new URL(context.viteOrigin).port, `${state.port}`);
  assert.equal((await fetch(context.viteOrigin, { signal: AbortSignal.timeout(2000) })).status, 404);
  running.child.kill('SIGINT');
  await waitFor(() => running.child.exitCode !== null, 'graceful shutdown');
  assert.equal(running.child.exitCode, 0, running.output());
  assert.equal(await readFile(state.stopped, 'utf8'), 'stopped');
  await assert.rejects(fetch(context.viteOrigin, { signal: AbortSignal.timeout(2000) }));
});

test('an occupied explicit Vite port fails before starting a backend', async t => {
  const state = await fixture(t);
  const running = start(t, state, true);
  await waitFor(() => running.child.exitCode !== null, 'explicit port failure');
  assert.notEqual(running.child.exitCode, 0);
  assert.match(running.output(), /already in use|EADDRINUSE/);
  await assert.rejects(readFile(state.ready), { code: 'ENOENT' });
});

test('an unexpected backend exit closes Vite and reports failure', async t => {
  const state = await fixture(t);
  const running = start(t, state, false);
  const context = await backendReady(state, running);
  assert.notEqual(new URL(context.viteOrigin).port, `${state.port}`);
  await writeFile(join(state.repository, 'crash'), 'crash');
  await waitFor(() => running.child.exitCode !== null, 'backend failure');
  assert.notEqual(running.child.exitCode, 0);
  await assert.rejects(fetch(context.viteOrigin, { signal: AbortSignal.timeout(2000) }));
});
