import { createHash } from 'node:crypto';
import { readFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';

const testFile = /(?:\.(?:test|spec)\.[cm]?[jt]s|\.dom\.test\.ts)$/;

async function files(root, path) {
  let entries;
  try { entries = await readdir(join(root, path), { withFileTypes: true }); }
  catch (error) { if (error.code === 'ENOENT') return []; throw error; }
  const result = [];
  for (const entry of entries) {
    if (entry.name.startsWith('.')) continue;
    const child = `${path}/${entry.name}`;
    if (entry.isDirectory()) result.push(...await files(root, child));
    else if (!testFile.test(entry.name)) result.push(child);
  }
  return result;
}

export async function uiFingerprint(root, environment = process.env, nodeVersion = process.version) {
  const paths = [
    'build.ts', 'package.json', 'package-lock.json', 'tsconfig.json', 'svelte.config.js', 'vite.config.ts',
    ...await files(root, 'src'), ...await files(root, 'public'), ...await files(root, 'build'),
    ...await files(root, '.generated/types'),
  ].sort();
  const digest = createHash('sha256').update(JSON.stringify({ version: 1, nodeVersion, nodeEnv: environment.NODE_ENV ?? '' }));
  for (const path of paths) {
    try {
      const bytes = await readFile(join(root, path));
      digest.update(JSON.stringify([path, bytes.length])).update(bytes);
    } catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  return digest.digest('hex');
}
