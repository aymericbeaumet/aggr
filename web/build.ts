import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { resolve, join } from 'node:path';
import { cachedUiBuild } from './build/cache.mjs';
import { ensureDependencies } from './build/dependencies.mjs';
import { withLock } from './build/lock.mjs';

const root = fileURLToPath(new URL('.', import.meta.url));
const destination = resolve(process.env.AGGR_UI_OUT_DIR ?? join(root, '.generated/dist'));
if (destination === root || root.startsWith(`${destination}/`)) throw new Error('UI output must not contain the source directory');
await ensureDependencies(root);
const result = await cachedUiBuild(root, destination, async (out: string) => {
  await ensureDependencies(root);
  // Keep package repair out of an active compiler and give each build fresh dependency imports.
  await withLock(join(root, '.generated/dependencies.lock'), () => new Promise<void>((accept, reject) => {
    const child = spawn(process.execPath, [join(root, 'build/compiler.ts')], {
      cwd: root,
      env: { ...process.env, AGGR_UI_OUT_DIR: out },
      stdio: 'inherit',
    });
    child.once('error', reject);
    child.once('exit', code => code === 0 ? accept() : reject(new Error(`Compiling the reader failed (${code}).`)));
  }));
});
console.log(result.hit ? 'Reader assets are current (cached).' : 'Reader assets built.');
