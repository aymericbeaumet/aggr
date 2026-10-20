import { createHash, randomUUID } from 'node:crypto';
import { cp, mkdir, readFile, readdir, rename, rm, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { uiFingerprint } from './inputs.mjs';
import { withLock } from './lock.mjs';

async function inventory(directory, prefix = '') {
  const result = [];
  for (const entry of await readdir(join(directory, prefix), { withFileTypes: true })) {
    const path = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) result.push(...await inventory(directory, path));
    else {
      const bytes = await readFile(join(directory, path));
      result.push({ path, hash: createHash('sha256').update(bytes).digest('hex') });
    }
  }
  return result.sort((a, b) => a.path.localeCompare(b.path));
}

async function matches(directory, manifest) {
  try { return JSON.stringify(await inventory(directory)) === JSON.stringify(manifest); }
  catch (error) { if (error.code === 'ENOENT') return false; throw error; }
}

async function complete(directory, manifest) {
  try {
    const assets = JSON.parse(await readFile(join(directory, 'assets.json'), 'utf8'));
    const paths = new Set(manifest.map(item => item.path));
    const required = ['renderer.js', 'assets.json', assets.app, ...Object.values(assets.files ?? {})];
    if (required.every(path => typeof path === 'string' && paths.has(path))) return;
  } catch (error) { if (error.code !== 'ENOENT' && !(error instanceof SyntaxError)) throw error; }
  throw new Error('Frontend build is incomplete: renderer or declared assets are missing.');
}

async function publish(source, destination, manifest) {
  if (await matches(destination, manifest)) return;
  await mkdir(dirname(destination), { recursive: true });
  const stage = `${destination}.staging-${randomUUID()}`;
  const previous = `${destination}.previous-${randomUUID()}`;
  await cp(source, stage, { recursive: true });
  let replaced = false;
  try {
    try { await rename(destination, previous); replaced = true; }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
    try { await rename(stage, destination); }
    catch (error) { if (replaced) await rename(previous, destination); throw error; }
  } finally {
    await rm(stage, { recursive: true, force: true });
    await rm(previous, { recursive: true, force: true });
  }
}

/** A cache entry becomes readable only after every artifact and its inventory are committed. */
export async function cachedUiBuild(root, destination, build, fingerprint = () => uiFingerprint(root)) {
  const cache = join(root, '.generated/ui-cache');
  return withLock(`${cache}.lock`, async () => {
    await mkdir(cache, { recursive: true });
    for (;;) {
      const key = await fingerprint();
      const entry = join(cache, key);
      let manifest;
      try { manifest = JSON.parse(await readFile(join(entry, 'complete.json'), 'utf8')); }
      catch (error) { if (error.code !== 'ENOENT' && !(error instanceof SyntaxError)) throw error; }
      let hit = Array.isArray(manifest) && await matches(join(entry, 'artifact'), manifest);
      if (!hit) {
        const stage = join(cache, `.staging-${randomUUID()}`);
        const artifact = join(stage, 'artifact');
        try {
          await mkdir(artifact, { recursive: true });
          await build(artifact);
          if (key !== await fingerprint()) continue;
          manifest = await inventory(artifact);
          await complete(artifact, manifest);
          await writeFile(join(stage, 'complete.json'), JSON.stringify(manifest));
          await rm(entry, { recursive: true, force: true });
          await rename(stage, entry);
        } finally { await rm(stage, { recursive: true, force: true }); }
      }
      await publish(join(entry, 'artifact'), destination, manifest);
      // Keep a bounded number of owned content-addressed generations.
      const generations = (await readdir(cache, { withFileTypes: true })).filter(item => item.isDirectory() && /^[a-f0-9]{64}$/.test(item.name));
      const obsolete = generations.filter(item => item.name !== key).slice(0, Math.max(0, generations.length - 4));
      for (const item of obsolete) await rm(join(cache, item.name), { recursive: true, force: true });
      return { hit: Boolean(hit), key };
    }
  });
}
