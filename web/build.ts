import { build } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { resolve, join, extname } from 'node:path';
import { mkdir, readFile, writeFile, readdir, rm, rename } from 'node:fs/promises';
import { licenses } from './build/licenses.ts';

const root = fileURLToPath(new URL('.', import.meta.url));
const out = resolve(process.env.AGGR_UI_OUT_DIR ?? join(root, '.generated/dist'));
const app = join(out, 'assets/app');
const common = { configFile: false as const, root, base: './', logLevel: 'warn' as const,
  resolve: { alias: { '@aggr-model': join(root, '.generated/types') } } };
if (out === root || root.startsWith(`${out}/`)) throw new Error('UI output must not contain the source directory');
await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });
await build({ ...common, plugins: [svelte(), licenses()], publicDir: false,
  build: { outDir: app, emptyOutDir: true, manifest: true, cssCodeSplit: false, target: 'es2022',
    rolldownOptions: { input: join(root, 'src/main.ts'), output: {
      entryFileNames: 'app-[hash].js', chunkFileNames: '[name]-[hash].js', assetFileNames: 'app-[hash][extname]' } } } });
for (const [entry, name] of [['bootstrap', 'bootstrap'], ['sw/main', 'worker']]) {
  await build({ ...common, publicDir: false, build: { outDir: app, emptyOutDir: false, target: 'es2022',
    lib: { entry: join(root, `src/${entry}.ts`), name: `Aggr${name}`, formats: ['iife'], fileName: () => `${name}.js` } } });
}
await build({ ...common, plugins: [svelte(), licenses('assets/renderer.LICENSE')], publicDir: 'public',
  ssr: { noExternal: true }, define: { 'process.env.NODE_ENV': '"production"' },
  build: { outDir: out, emptyOutDir: false, target: 'es2022', ssr: true,
    lib: { entry: join(root, 'src/embedded.ts'), name: 'AggrRenderer', formats: ['iife'], fileName: () => 'renderer.js' },
    rolldownOptions: { output: { entryFileNames: 'renderer.js', codeSplitting: false } } } });
const manifest = JSON.parse(await readFile(join(app, '.vite/manifest.json'), 'utf8'));
const entry = manifest['src/main.ts'];
const path = (value: string) => `assets/app/${value}`;
const imported = new Set<string>();
function visit(key: string) { if (imported.has(key)) return; imported.add(key); for (const child of manifest[key].imports ?? []) visit(child); }
for (const key of entry.imports ?? []) visit(key);
const imports = [...imported].map((key) => path(manifest[key].file));
const files: Record<string, string> = {};
async function collect(directory: string, prefix = '') {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const name = prefix + entry.name;
    if (entry.isDirectory()) { if (!entry.name.startsWith('.')) await collect(join(directory, entry.name), `${name}/`); }
    else if (!['renderer.js', 'assets.json'].includes(name)) files[name] = name;
  }
}
await collect(out);
// Authored files have logical names in page models and immutable content-addressed URLs.
async function hashFile(logical: string) {
  const bytes = await readFile(join(out, logical));
  const extension = extname(logical);
  const stem = extension ? logical.slice(0, -extension.length) : logical;
  const physical = `${stem}-${createHash('sha256').update(bytes).digest('hex').slice(0, 12)}${extension}`;
  await rename(join(out, logical), join(out, physical));
  files[logical] = physical;
}
async function hashPublic(directory: string, prefix = '') {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const logical = prefix + entry.name;
    if (entry.isDirectory()) await hashPublic(join(directory, entry.name), `${logical}/`);
    else await hashFile(logical);
  }
}
await hashFile(path('bootstrap.js'));
await hashFile(path('worker.js'));
await hashPublic(join(root, 'public'));
await writeFile(join(out, 'assets.json'), JSON.stringify({ app: path(entry.file), css: manifest['style.css'] ? path(manifest['style.css'].file) : entry.css?.[0] ? path(entry.css[0]) : null, imports,
  bootstrap: files[path('bootstrap.js')], worker: files[path('worker.js')], files }));
