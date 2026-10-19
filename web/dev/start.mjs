import { request as httpRequest } from 'node:http';
import { join } from 'node:path';
import { build, createServer } from 'vite';
import { Backend } from './backend.mjs';
import { needsBuild, readContext } from './context.mjs';

const context = readContext();
const origin = new URL(context.viteOrigin);
const release = context.workerArgs.includes('--release');
let backend;
let frontend;
let stopping;
let finish;
let bootstrap;
const bootstrapInputs = new Set();
const finished = new Promise(resolve => { finish = resolve; });

async function bootstrapCode() {
  if (!bootstrap) {
    bootstrap = build({
      configFile: false,
      root: join(context.repository, 'web'),
      publicDir: false,
      logLevel: 'error',
      build: {
        write: false,
        minify: true,
        target: 'es2022',
        lib: { entry: join(context.repository, 'web/src/bootstrap.ts'), name: 'AggrBootstrap', formats: ['iife'] },
      },
    }).then(result => {
      const outputs = Array.isArray(result) ? result : [result];
      const chunk = outputs.flatMap(output => output.output).find(output => output.type === 'chunk' && output.isEntry);
      if (!chunk) throw new Error('The preference bootstrap did not produce a script.');
      bootstrapInputs.clear();
      for (const file of Object.keys(chunk.modules)) bootstrapInputs.add(file);
      return chunk.code;
    }).catch(error => { bootstrap = undefined; throw error; });
  }
  return bootstrap;
}

function waiting(response) {
  response.writeHead(503, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store', 'Retry-After': '1' });
  response.end('<!doctype html><meta charset="utf-8"><title>aggr dev</title><p>Preparing the reader…</p><script>setTimeout(()=>location.reload(),1000)</script>');
}

async function renderPage(request, upstream, response) {
  const chunks = [];
  for await (const chunk of upstream) chunks.push(chunk);
  const model = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  model.page.site.assets = {
    ...model.page.site.assets,
    app: `${origin.origin}/src/main.ts`,
    css: `${origin.origin}/src/style.css`,
    imports: [],
  };
  if (model.page.head) model.page.head.bootstrap = `${origin.origin}/@aggr/bootstrap.js`;
  const renderer = await frontend.ssrLoadModule('/src/server.ts');
  let html = renderer.render(model);
  const reload = upstream.headers['x-aggr-dev-client'];
  if (typeof reload === 'string') {
    const script = `<script src="${reload.replaceAll('&', '&amp;').replaceAll('"', '&quot;')}"></script>`;
    html = html.includes('</body>') ? html.replace('</body>', `${script}</body>`) : html + script;
  }
  html = await frontend.transformIndexHtml(request.url, html);
  response.writeHead(upstream.statusCode ?? 200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' });
  response.end(request.method === 'HEAD' ? undefined : html);
}

function proxy(request, response) {
  if (request.url.split('?')[0] === '/@aggr/bootstrap.js') {
    bootstrapCode().then(code => {
      response.writeHead(200, { 'Content-Type': 'text/javascript; charset=utf-8', 'Cache-Control': 'no-store' });
      response.end(request.method === 'HEAD' ? undefined : code);
    }).catch(error => {
      console.error(error);
      response.writeHead(500, { 'Content-Type': 'text/plain; charset=utf-8' });
      response.end('Could not compile the preference bootstrap.');
    });
    return;
  }
  if (!backend?.origin) { waiting(response); return; }
  const headers = { ...request.headers, host: new URL(backend.origin).host };
  delete headers['x-aggr-page-model'];
  if (!release) headers['x-aggr-page-model'] = '1';
  const upstream = httpRequest(backend.origin, {
    path: request.url,
    method: request.method === 'HEAD' ? 'GET' : request.method,
    headers,
  }, result => {
    if (!release && result.headers['x-aggr-page-model'] === '1') {
      renderPage(request, result, response).catch(error => {
        frontend.ssrFixStacktrace(error);
        console.error(error);
        if (!response.headersSent) response.writeHead(500, { 'Content-Type': 'text/plain; charset=utf-8', 'Cache-Control': 'no-store' });
        response.end('The page could not be rendered. See the development server log.');
      });
      return;
    }
    response.writeHead(result.statusCode ?? 502, result.headers);
    if (request.method === 'HEAD') { result.resume(); response.end(); }
    else result.pipe(response);
  });
  upstream.on('error', error => {
    if (stopping || response.destroyed) return;
    if (!response.headersSent) waiting(response);
    else response.destroy(error);
  });
  response.on('close', () => upstream.destroy());
  request.pipe(upstream);
}

async function stop(error) {
  if (stopping) return stopping;
  if (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
  stopping = (async () => {
    await backend?.close();
    await frontend?.close();
    finish();
  })();
  return stopping;
}

process.once('SIGINT', () => { void stop(); });
process.once('SIGTERM', () => { void stop(); });

async function run() {
  frontend = await createServer({
    root: join(context.repository, 'web'),
    appType: 'custom',
    server: {
      host: origin.hostname.replace(/^\[|\]$/g, ''),
      port: Number(origin.port || 80),
      strictPort: context.strictPort ?? false,
    },
    plugins: [{
      name: 'aggr-development-pages',
      configureServer(server) { return () => server.middlewares.use(proxy); },
    }],
  });
  if (stopping) { await frontend.close(); return; }
  await frontend.listen();
  if (stopping) { await frontend.close(); return; }
  const address = frontend.httpServer.address();
  origin.port = String(address.port);
  frontend.config.server.origin = origin.origin;
  backend = new Backend({ ...context, viteOrigin: origin.origin }, {
    ready(worker) {
      console.log(`aggr dev: ${origin.origin}${worker.base}`);
      frontend.ws.send({ type: 'full-reload' });
    },
    exit(error) { void stop(error); },
    buildError(error) {
      frontend.ws.send({ type: 'error', err: { message: error.message, stack: error.stack ?? '' } });
    },
  });
  frontend.watcher.add([
    join(context.repository, 'src'), join(context.repository, 'crates'),
    join(context.repository, 'Cargo.toml'), join(context.repository, 'Cargo.lock'),
    join(context.repository, 'build.rs'), join(context.repository, '.cargo/config.toml'),
  ]);
  frontend.watcher.on('all', (event, file) => {
    if (['add', 'change', 'unlink'].includes(event) && needsBuild(context, file)) backend.rebuild();
    if (bootstrapInputs.has(file)) {
      bootstrap = undefined;
      frontend.ws.send({ type: 'full-reload' });
    }
  });
  await backend.start();
  await finished;
}

try {
  await run();
} catch (error) {
  await stop(error);
} finally {
  await stop();
}
