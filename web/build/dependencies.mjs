import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { withLock } from './lock.mjs';

const matchesPlatform = (values, value) => !values || (!values.includes(`!${value}`) && (values.every(item => item.startsWith('!')) || values.includes(value)));

async function installed(root, lock) {
  const libc = process.platform === 'linux' ? (process.report.getReport().header.glibcVersionRuntime ? 'glibc' : 'musl') : '';
  for (const [path, pkg] of Object.entries(lock.packages ?? {})) {
    if (!path || !matchesPlatform(pkg.os, process.platform) || !matchesPlatform(pkg.cpu, process.arch) || !matchesPlatform(pkg.libc, libc)) continue;
    try {
      const actual = JSON.parse(await readFile(join(root, path, 'package.json'), 'utf8'));
      if (actual.version !== pkg.version) return false;
    } catch (error) {
      if (error.code === 'ENOENT' || error instanceof SyntaxError) return false;
      throw error;
    }
  }
  return true;
}

function npmCi(root) {
  return new Promise((accept, reject) => {
    const child = spawn(process.platform === 'win32' ? 'npm.cmd' : 'npm', ['ci', '--include=dev', '--no-audit', '--no-fund'], { cwd: root, stdio: 'inherit', shell: process.platform === 'win32' });
    child.once('error', reject);
    child.once('exit', code => code === 0 ? accept() : reject(new Error(`Installing locked frontend dependencies failed (${code}). Run make setup to retry.`)));
  });
}

/** Explicitly callable even when Cargo considers its previous build fresh. */
export async function ensureDependencies(root, install = npmCi) {
  return withLock(join(root, '.generated/dependencies.lock'), async () => {
    const packageBytes = await readFile(join(root, 'package.json'));
    const lockBytes = await readFile(join(root, 'package-lock.json'));
    const lock = JSON.parse(lockBytes);
    const digest = createHash('sha256').update(packageBytes).update(lockBytes).update(JSON.stringify([process.version, process.platform, process.arch])).digest('hex');
    const receipt = join(root, 'node_modules/.aggr-dependencies');
    let previous;
    try { previous = await readFile(receipt, 'utf8'); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
    if (previous === digest && await installed(root, lock)) return false;
    await install(root);
    if (!await installed(root, lock)) throw new Error('Frontend dependencies are incomplete after npm ci. Run make setup to retry.');
    await mkdir(dirname(receipt), { recursive: true });
    const stage = `${receipt}.${randomUUID()}`;
    await writeFile(stage, digest);
    await rename(stage, receipt);
    return true;
  });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await ensureDependencies(fileURLToPath(new URL('..', import.meta.url)));
}
