import { randomUUID } from 'node:crypto';
import { mkdir, readFile, rename, rm, stat, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

async function owner(directory) {
  try { return JSON.parse(await readFile(join(directory, 'owner.json'), 'utf8')); }
  catch { return null; }
}

function alive(pid) {
  try { process.kill(pid, 0); return true; }
  catch (error) { return error.code !== 'ESRCH'; }
}

async function abandoned(directory, observed) {
  if (observed) return !alive(observed.pid);
  try { return Date.now() - (await stat(directory)).mtimeMs > 30_000; }
  catch { return false; }
}

async function recover(directory) {
  const observed = await owner(directory);
  if (!await abandoned(directory, observed)) return;
  const recovery = join(directory, 'recovery');
  try { await mkdir(recovery); }
  catch (error) { if (['EEXIST', 'ENOENT'].includes(error.code)) return; throw error; }
  try {
    // Recheck after claiming recovery: another contender may already have replaced the lock.
    const current = await owner(directory);
    if (current?.token !== observed?.token || !await abandoned(directory, current)) return;
    const stale = `${directory}.abandoned-${randomUUID()}`;
    await rename(directory, stale);
    await rm(stale, { recursive: true, force: true });
  } finally {
    if ((await owner(directory))?.token === observed?.token) await rm(recovery, { recursive: true, force: true });
  }
}

/** Cross-process serialization for owned generated directories; interrupted owners are recoverable. */
export async function withLock(directory, run) {
  await mkdir(dirname(directory), { recursive: true });
  const token = randomUUID();
  const started = Date.now();
  for (;;) {
    try { await mkdir(directory); break; }
    catch (error) {
      if (error.code !== 'EEXIST') throw error;
      await recover(directory);
      if (Date.now() - started > 300_000) throw new Error(`Timed out waiting for another frontend build: ${directory}`);
      await delay(25);
    }
  }
  try {
    await writeFile(join(directory, 'owner.json'), JSON.stringify({ pid: process.pid, token }));
    return await run();
  } finally {
    if ((await owner(directory))?.token === token) await rm(directory, { recursive: true, force: true });
  }
}
