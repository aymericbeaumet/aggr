import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { EventEmitter } from 'node:events';
import test from 'node:test';
import { LastGoodResponses, ReadinessGate } from './responses.mjs';

async function fixture(t, options) {
  const cache = new LastGoodResponses(options);
  const state = { available: true, body: 'old reader', finish: null };
  const server = createServer((request, response) => {
    cache.capture(request, response);
    if (!state.available) {
      if (!cache.respond(request, response)) { response.writeHead(503); response.end('preparing'); }
      return;
    }
    const type = request.url === '/events' ? 'text/event-stream' : request.url.endsWith('.png') ? 'image/png' : 'text/html';
    response.writeHead(request.url === '/error' ? 500 : 200, { 'content-type': type });
    if (request.url === '/late') {
      response.write('started');
      state.finish = () => response.end(' finished');
    } else if (request.url === '/partial') {
      response.write('started');
      response.destroy();
    } else response.end(request.url === '/' ? state.body : request.url);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => { server.closeAllConnections(); server.close(resolve); }));
  return { cache, state, fetch: (path = '/', options) => fetch(`http://127.0.0.1:${server.address().port}${path}`, options) };
}

test('visited pages and binary assets stay available throughout a delayed worker handoff', async t => {
  const app = await fixture(t);
  assert.equal(await (await app.fetch()).text(), 'old reader');
  const asset = await (await app.fetch('/picture.png')).arrayBuffer();
  app.state.available = false;
  const page = await app.fetch();
  assert.equal(page.status, 200);
  assert.equal(await page.text(), 'old reader');
  assert.deepEqual(await (await app.fetch('/picture.png')).arrayBuffer(), asset);
  assert.equal((await app.fetch('/unvisited')).status, 503);
  app.cache.clear();
  assert.equal((await app.fetch()).status, 503, 'promotion discards old generation responses');
  app.state.available = true;
  app.state.body = 'new reader';
  assert.equal(await (await app.fetch()).text(), 'new reader');
});

test('failed, partial, streaming, control and non-GET responses are never retained', async t => {
  const app = await fixture(t);
  for (const path of ['/error', '/events', '/__aggr/dev.js']) await (await app.fetch(path)).text();
  await (await app.fetch('/post', { method: 'POST' })).text();
  await assert.rejects(app.fetch('/partial').then(response => response.text()));
  app.state.available = false;
  for (const path of ['/error', '/events', '/__aggr/dev.js', '/post', '/partial']) assert.equal((await app.fetch(path)).status, 503, path);
});

test('retention obeys entry, total byte and individual response bounds', async t => {
  const app = await fixture(t, { maxEntries: 2, maxBytes: 64, maxEntryBytes: 4 });
  for (const path of ['/a', '/b', '/c', '/too-long']) await (await app.fetch(path)).text();
  app.state.available = false;
  assert.equal((await app.fetch('/a')).status, 503);
  assert.equal((await app.fetch('/b')).status, 200);
  assert.equal((await app.fetch('/c')).status, 200);
  assert.equal((await app.fetch('/too-long')).status, 503);
});

test('total retained bytes evict old entries even before the count limit', async t => {
  const app = await fixture(t, { maxEntries: 10, maxBytes: 4, maxEntryBytes: 4 });
  for (const path of ['/a', '/b', '/c']) await (await app.fetch(path)).text();
  app.state.available = false;
  assert.equal((await app.fetch('/a')).status, 503);
  assert.equal((await app.fetch('/b')).status, 200);
  assert.equal((await app.fetch('/c')).status, 200);
});

test('a previous generation finishing after promotion cannot repopulate the fallback', async t => {
  const app = await fixture(t);
  const pending = app.fetch('/late').then(response => response.text());
  while (!app.state.finish) await new Promise(resolve => setTimeout(resolve, 5));
  app.cache.clear();
  app.state.finish();
  await pending;
  app.state.available = false;
  assert.equal((await app.fetch('/late')).status, 503);
});

function navigation(path = '/', headers = { accept: 'text/html' }) {
  const request = Object.assign(new EventEmitter(), { method: 'GET', url: path, headers });
  const response = Object.assign(new EventEmitter(), { destroyed: false });
  return { request, response };
}

test('uncached document and in-app page requests wait until replacement readiness', async () => {
  const gate = new ReadinessGate();
  const document = navigation('/new-page/');
  const article = navigation('/items/example/unseen/', { accept: '*/*' });
  const first = gate.wait(document.request, document.response);
  const second = gate.wait(article.request, article.response);
  assert(first instanceof Promise);
  assert(second instanceof Promise);
  gate.ready('/');
  assert.equal(await first, true);
  assert.equal(await second, true);
  assert.equal(document.response.listenerCount('close'), 0);
  gate.close();
});

test('waiting is bounded and aborts release capacity and event listeners', async () => {
  const gate = new ReadinessGate({ maxPending: 1, timeoutMs: 25 });
  const first = navigation();
  const waiting = gate.wait(first.request, first.response);
  assert(waiting instanceof Promise);
  const full = navigation('/other/');
  assert.equal(gate.wait(full.request, full.response), null);
  first.response.emit('close');
  assert.equal(await waiting, false);
  assert.equal(first.request.listenerCount('aborted'), 0);
  const timed = gate.wait(full.request, full.response);
  assert(timed instanceof Promise);
  assert.equal(await timed, false);
  const final = navigation('/items/next/', { accept: '*/*' });
  const stopped = gate.wait(final.request, final.response);
  assert(stopped instanceof Promise);
  gate.close();
  assert.equal(await stopped, false);
  assert.equal(final.response.listenerCount('close'), 0);
  assert.equal(gate.wait(final.request, final.response), null);
});

test('control, SSE, range, media and unrelated API requests bypass readiness waiting', () => {
  const gate = new ReadinessGate();
  for (const [path, headers] of [
    ['/__aggr/reload', { accept: 'text/html' }],
    ['/events/', { accept: 'text/event-stream' }],
    ['/items/one/', { accept: 'text/html', range: 'bytes=0-10' }],
    ['/media.webm', { accept: 'text/html' }],
    ['/updates.json', { accept: '*/*' }],
    ['/api/events/', { accept: '*/*' }],
  ]) {
    const { request, response } = navigation(path, headers);
    assert.equal(gate.wait(request, response), null, path);
  }
  gate.close();
});
