import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { Backend } from './backend.mjs';

const entry = fileURLToPath(new URL('./start.mjs', import.meta.url));
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));

async function waitFor(check, label) {
  const deadline = Date.now() + 45000;
  while (Date.now() < deadline) {
    if (await check()) return;
    await delay(25);
  }
  throw new Error(`${label} timed out`);
}

async function fixture(t) {
  const repository = await mkdtemp(join(tmpdir(), 'aggr-dev-lifecycle-'));
  await mkdir(join(repository, 'web/src'), { recursive: true });
  await writeFile(join(repository, 'web/src/server.ts'), `export function render(request) { return '<!doctype html><html><body><h1>' + request.body_html + '</h1><img src="' + request.page.site.assets.icon + '"><script type="module" src="' + request.page.site.assets.app + '"></script></body></html>'; }`);
  const executable = join(repository, 'worker.mjs');
  const ready = join(repository, 'ready.json');
  const stopped = join(repository, 'stopped');
  const cleanups = [];
  await writeFile(join(repository, 'web/src/bootstrap.ts'), "document.documentElement.dataset.preference = 'first';");
  await writeFile(executable, `#!${process.execPath}
import { createServer } from 'node:http';
import { watch, writeFileSync, existsSync } from 'node:fs';
const directory = process.cwd();
const server = createServer((request, response) => {
  if (request.headers['x-aggr-page-model'] === '1' && request.url === '/') {
    response.writeHead(200, { 'content-type': 'application/json', 'x-aggr-page-model': '1', 'x-aggr-dev-client': '/__aggr/dev.js?run=1&revision=1' });
    response.end(JSON.stringify({ page: { site: { assets: { icon: '/assets/reader-icon.png' } } }, body_html: 'Source renderer' }));
  } else { response.end('Worker ' + process.pid); }
});
server.listen(0, '127.0.0.1', () => {
  const origin = 'http://127.0.0.1:' + server.address().port;
  writeFileSync(directory + '/ready.json', JSON.stringify({ pid: process.pid, origin, publicOrigin: process.env.AGGR_DEV_PUBLIC_ORIGIN, restarted: process.env.AGGR_DEV_RESTART, arguments: process.argv.slice(2), executable: process.argv[1] }));
  console.log('\\x1eaggr-ready ' + JSON.stringify({ origin, base: '/' }));
});
process.stdin.once('data', () => { writeFileSync(directory + '/stopped', String(process.pid)); server.close(() => process.exit(0)); });
process.stdin.once('end', () => server.close(() => process.exit(0)));
watch(directory, () => { if (existsSync(directory + '/crash')) process.exit(7); });
`);
  await chmod(executable, 0o755);
  const occupied = createServer();
  await new Promise(resolve => occupied.listen(0, '127.0.0.1', resolve));
  t.after(async () => {
    for (const cleanup of cleanups) await cleanup();
    await new Promise(resolve => occupied.close(resolve));
    await rm(repository, { recursive: true, force: true });
  });
  return { repository, executable, ready, stopped, cleanups, port: occupied.address().port };
}

function context(state, strictPort = false) {
  return { repository: state.repository, cwd: state.repository, executable: state.executable, workerArgs: ['dev', '--clean', '--port', String(state.port)], viteOrigin: `http://127.0.0.1:${state.port}`, strictPort };
}

function start(t, state, strictPort) {
  const child = spawn(process.execPath, [entry], {
    env: { ...process.env, AGGR_DEV_CONTEXT: JSON.stringify(context(state, strictPort)) },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '';
  child.stdout.on('data', chunk => { output += chunk; });
  child.stderr.on('data', chunk => { output += chunk; });
  state.cleanups.push(async () => {
    if (child.exitCode === null && child.signalCode === null) {
      child.kill('SIGINT');
      await waitFor(() => child.exitCode !== null || child.signalCode !== null, 'test process cleanup');
    }
  });
  return { child, output: () => output };
}

async function backendReady(state, running) {
  let info;
  await waitFor(async () => {
    assert.equal(running.child.exitCode, null, running.output());
    try { info = JSON.parse(await readFile(state.ready, 'utf8')); return true; }
    catch (error) { if (error.code === 'ENOENT') return false; throw error; }
  }, 'backend readiness');
  return info;
}

test('one stable public port serves shared SSR and shutdown stops the worker', { skip: process.platform === 'win32' }, async t => {
  const state = await fixture(t);
  const running = start(t, state, false);
  const ready = await backendReady(state, running);
  assert.notEqual(new URL(ready.publicOrigin).port, `${state.port}`);
  let html;
  await waitFor(async () => {
    const response = await fetch(ready.publicOrigin);
    html = await response.text();
    return response.status === 200;
  }, 'public reader');
  assert.match(html, /<h1>Source renderer<\/h1>/);
  assert.match(html, /@vite\/client/);
  assert.match(html, /src\/main\.ts/);
  assert.match(html, /<img src="\/assets\/reader-icon\.png">/);
  assert.match(html, /__aggr\/dev\.js\?run=1&amp;revision=1/);
  assert.notEqual(ready.executable, state.executable);
  const doubleSlash = await fetch(`${ready.publicOrigin}//127.0.0.1:1/`);
  assert.equal(await doubleSlash.text(), `Worker ${ready.pid}`);
  const bootstrapUrl = `${ready.publicOrigin}/@aggr/bootstrap.js`;
  const script = await (await fetch(bootstrapUrl)).text();
  assert.match(script, /first/, running.output());
  assert(!script.includes('import '));
  await writeFile(join(state.repository, 'web/src/bootstrap.ts'), "document.documentElement.dataset.preference = 'second';");
  await waitFor(async () => (await (await fetch(bootstrapUrl)).text()).includes('second'), 'bootstrap hot reload');
  running.child.kill('SIGINT');
  await waitFor(() => running.child.exitCode !== null, 'graceful shutdown');
  assert.equal(running.child.exitCode, 0, running.output());
  assert.equal(await readFile(state.stopped, 'utf8'), String(ready.pid));
  await assert.rejects(fetch(ready.publicOrigin, { signal: AbortSignal.timeout(2000) }));
});

test('an occupied explicit public port fails before starting a backend', async t => {
  const state = await fixture(t);
  const running = start(t, state, true);
  await waitFor(() => running.child.exitCode !== null, 'explicit port failure');
  assert.notEqual(running.child.exitCode, 0);
  assert.match(running.output(), /already in use|EADDRINUSE/);
  await assert.rejects(readFile(state.ready), { code: 'ENOENT' });
});

test('an unexpected worker exit closes the public server and reports failure', { skip: process.platform === 'win32' }, async t => {
  const state = await fixture(t);
  const running = start(t, state, false);
  const ready = await backendReady(state, running);
  await writeFile(join(state.repository, 'crash'), 'crash');
  await waitFor(() => running.child.exitCode !== null, 'backend failure');
  assert.notEqual(running.child.exitCode, 0);
  await assert.rejects(fetch(ready.publicOrigin, { signal: AbortSignal.timeout(2000) }));
});

test('failed compiles retain the worker and successful rebuilds reuse data without cleaning', { skip: process.platform === 'win32' }, async t => {
  const state = await fixture(t);
  let builds = 0;
  let failures = 0;
  let starts = 0;
  const backend = new Backend(context(state), {
    compile() { builds++; if (builds === 1) throw new Error('fixture compiler failure'); return state.executable; },
    buildError() { failures++; },
    ready() { starts++; },
  });
  state.cleanups.push(() => backend.close());
  await backend.start();
  const first = JSON.parse(await readFile(state.ready, 'utf8'));
  backend.rebuild();
  await waitFor(() => failures === 1, 'failed compilation');
  assert.equal(backend.origin, first.origin);
  assert.equal(await (await fetch(backend.origin)).text(), `Worker ${first.pid}`);
  backend.rebuild();
  await waitFor(() => starts === 2, 'successful restart');
  const next = JSON.parse(await readFile(state.ready, 'utf8'));
  assert.notEqual(next.pid, first.pid);
  assert.notEqual(next.executable, first.executable);
  assert.equal(next.restarted, '1');
  assert(!next.arguments.includes('--clean'));
  assert.equal(await readFile(state.stopped, 'utf8'), String(first.pid));
});

test('a worker that fails during startup restores the previous executable', { skip: process.platform === 'win32' }, async t => {
  const state = await fixture(t);
  const broken = join(state.repository, 'broken.mjs');
  await writeFile(broken, `#!${process.execPath}\nprocess.exit(7);\n`);
  await chmod(broken, 0o755);
  let failed = false;
  const backend = new Backend(context(state), {
    compile: () => broken,
    buildError: () => { failed = true; },
  });
  state.cleanups.push(() => backend.close());
  await backend.start();
  const first = JSON.parse(await readFile(state.ready, 'utf8'));
  backend.rebuild();
  await waitFor(() => failed, 'startup rollback');
  const restored = JSON.parse(await readFile(state.ready, 'utf8'));
  assert.equal(restored.executable, first.executable);
  assert.notEqual(restored.pid, first.pid);
  assert.equal(restored.restarted, '1');
  assert.equal(await (await fetch(backend.origin)).text(), `Worker ${restored.pid}`);
});

test('shutdown cancels a worker that has not reported readiness', { skip: process.platform === 'win32' }, async t => {
  const state = await fixture(t);
  await writeFile(state.executable, `#!${process.execPath}\nimport { writeFileSync } from 'node:fs';\nwriteFileSync('started', 'yes');\nprocess.stdin.once('data', () => process.exit(0));\n`);
  const backend = new Backend(context(state));
  state.cleanups.push(() => backend.close());
  const starting = backend.start();
  const rejected = assert.rejects(starting, /exited before readiness|stopping/);
  await waitFor(async () => {
    try { return await readFile(join(state.repository, 'started'), 'utf8') === 'yes'; }
    catch { return false; }
  }, 'starting worker');
  await backend.close();
  await rejected;
});
