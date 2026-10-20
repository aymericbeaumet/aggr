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

function start(t, state, strictPort, environment = {}) {
  const child = spawn(process.execPath, [entry], {
    env: { ...process.env, ...environment, AGGR_DEV_CONTEXT: JSON.stringify(context(state, strictPort)) },
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

test('the public reader serves visited pages and assets while a replacement waits for readiness', { skip: process.platform === 'win32' }, async t => {
  const state = await fixture(t);
  const release = join(state.repository, 'release');
  state.cleanups.push(() => writeFile(release, 'ready'));
  const source = await readFile(state.executable, 'utf8');
  const preparation = `const cancelPreparation = () => process.exit(0);
if (process.env.AGGR_DEV_RESTART === '1') process.stdin.once('data', cancelPreparation);
while (process.env.AGGR_DEV_RESTART === '1' && !existsSync(directory + '/release')) await new Promise(resolve => setTimeout(resolve, 10));
process.stdin.removeListener('data', cancelPreparation);
`;
  await writeFile(state.executable, source.replace("server.listen(0, '127.0.0.1'", `${preparation}server.listen(0, '127.0.0.1'`));
  const tools = join(state.repository, 'tools');
  await mkdir(tools);
  await mkdir(join(state.repository, 'src'));
  const rust = join(state.repository, 'src/fixture.rs');
  await writeFile(rust, 'initial');
  const cargo = join(tools, 'cargo');
  await writeFile(cargo, `#!${process.execPath}\nconsole.log(JSON.stringify({reason:'compiler-artifact',target:{name:'aggr-dev-worker'},executable:process.env.AGGR_FIXTURE_EXECUTABLE}));\n`);
  await chmod(cargo, 0o755);
  const report = join(state.repository, 'reports/dev-timings.jsonl');
  const running = start(t, state, false, { PATH: `${tools}:${process.env.PATH}`, AGGR_FIXTURE_EXECUTABLE: state.executable, AGGR_DEV_TIMINGS_REPORT: 'reports/dev-timings.jsonl' });
  const initial = await backendReady(state, running);
  const page = await (await fetch(initial.publicOrigin)).text();
  const asset = await (await fetch(`${initial.publicOrigin}/assets/fixture.png`)).text();
  await writeFile(rust, 'updated');
  await waitFor(async () => {
    try { return await readFile(state.stopped, 'utf8') === String(initial.pid); }
    catch { return false; }
  }, 'old worker shutdown');
  const pending = await fetch(initial.publicOrigin);
  assert.equal(pending.status, 200, running.output());
  assert.equal(await pending.text(), page);
  assert.equal(await (await fetch(`${initial.publicOrigin}/assets/fixture.png`)).text(), asset);
  const arrived = [false, false];
  const unseen = [
    fetch(`${initial.publicOrigin}/items/example/unseen/`),
    fetch(`${initial.publicOrigin}/items/example/progressive/`, { headers: { accept: 'text/html' } }),
  ].map((request, index) => request.then(response => { arrived[index] = true; return response; }));
  const controller = new AbortController();
  const aborted = assert.rejects(fetch(`${initial.publicOrigin}/items/example/aborted/`, { signal: controller.signal }), { name: 'AbortError' });
  await delay(50);
  assert.deepEqual(arrived, [false, false], 'uncached page requests wait without an interim 503');
  controller.abort();
  await aborted;
  await writeFile(release, 'ready');
  await waitFor(async () => {
    const current = JSON.parse(await readFile(state.ready, 'utf8'));
    return current.pid !== initial.pid && await (await fetch(`${initial.publicOrigin}/assets/fixture.png`)).text() === `Worker ${current.pid}`;
  }, 'replacement promotion');
  const replacement = JSON.parse(await readFile(state.ready, 'utf8'));
  for (const request of unseen) {
    const response = await request;
    assert.equal(response.status, 200);
    assert.equal(await response.text(), `Worker ${replacement.pid}`);
  }
  const records = async () => {
    try { return (await readFile(report, 'utf8')).trim().split('\n').map(line => JSON.parse(line)); }
    catch (error) { if (error.code === 'ENOENT') return []; throw error; }
  };
  await waitFor(async () => (await records()).length === 1, 'first timing record');
  await writeFile(rust, 'updated again');
  await waitFor(async () => (await records()).length === 2, 'appended timing record');
  const measured = await records();
  assert(measured.every(record => record.status === 'success' && record.compileMs >= 0 && record.prepareMs > 0));
  assert(Date.parse(measured[0].startedAt) <= Date.parse(measured[1].startedAt));
  assert.match(running.output(), /dev build: compile [\d.]+s, prepare [\d.]+s \(success\)/);
  const current = JSON.parse(await readFile(state.ready, 'utf8'));
  await rm(release);
  await writeFile(rust, 'stop during preparation');
  await waitFor(async () => await readFile(state.stopped, 'utf8') === String(current.pid), 'last worker shutdown');
  const shutdown = fetch(`${initial.publicOrigin}/items/example/shutdown/`, { signal: AbortSignal.timeout(5000) });
  await delay(50);
  running.child.kill('SIGINT');
  const stopped = await shutdown;
  assert.equal(stopped.status, 503);
  assert.match(await stopped.text(), /stopping/);
  await waitFor(() => running.child.exitCode !== null, 'shutdown with a waiting navigation');
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
  let measurement;
  const backend = new Backend(context(state), {
    compile: () => broken,
    buildError: () => { failed = true; },
    metrics: next => { measurement = next; },
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
  assert.equal(measurement.status, 'failed');
  assert.equal(measurement.phase, 'prepare');
  assert(measurement.compileMs >= 0 && measurement.prepareMs > 0);
});

for (const latestFails of [false, true]) {
  test(`a superseded compile is never promoted when the latest compile ${latestFails ? 'fails' : 'succeeds'}`, { skip: process.platform === 'win32' }, async t => {
    const state = await fixture(t);
    const compiles = [];
    const metrics = [];
    let starts = 0;
    let failures = 0;
    const backend = new Backend(context(state), {
      compile: () => new Promise((resolve, reject) => compiles.push({ resolve, reject })),
      ready: () => { starts++; },
      buildError: () => { failures++; },
      metrics: measurement => metrics.push(measurement),
    });
    state.cleanups.push(async () => {
      for (const compile of compiles) compile.reject(new Error('test cleanup'));
      await backend.close();
    });
    await backend.start();
    const original = backend.origin;
    backend.rebuild();
    await waitFor(() => compiles.length === 1, 'first compile');
    backend.rebuild();
    compiles[0].resolve(state.executable);
    await waitFor(() => compiles.length === 2, 'latest compile');
    assert.equal(metrics.length, 1);
    assert.equal(metrics[0].status, 'superseded');
    assert.equal(metrics[0].prepareMs, null);
    assert(metrics[0].compileMs >= 0);
    assert.equal(starts, 1, 'the superseded artifact must not restart the worker');
    assert.equal(backend.origin, original);
    assert.equal((await fetch(original)).status, 200);
    if (latestFails) {
      compiles[1].reject(new Error('latest fixture compile failed'));
      await waitFor(() => failures === 1, 'latest compilation failure');
      assert.equal(starts, 1);
      assert.equal(backend.origin, original);
    } else {
      compiles[1].resolve(state.executable);
      await waitFor(() => starts === 2, 'latest artifact promotion');
      assert.notEqual(backend.origin, original);
    }
    await waitFor(() => metrics.length === 2, 'latest compile metrics');
    assert.equal(metrics[1].status, latestFails ? 'failed' : 'success');
    assert.equal(metrics[1].phase, latestFails ? 'compile' : 'prepare');
    assert(metrics[1].compileMs >= 0);
    assert(latestFails ? metrics[1].prepareMs === null : metrics[1].prepareMs > 0);
  });
}

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
