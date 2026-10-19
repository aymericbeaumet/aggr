import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const filename = process.platform === 'win32' ? 'aggr.exe' : 'aggr';
const binary = resolve(process.argv[2] ?? fileURLToPath(new URL(`../../target/release/${filename}`, import.meta.url)));
const helpResult = spawnSync(binary, ['--help'], { encoding: 'utf8' });
assert.equal(helpResult.status, 0, helpResult.error?.message ?? helpResult.stderr);
const help = helpResult.stdout;
assert(!/^\s+dev(?:\s|$)/m.test(help), 'release help must not advertise dev');
assert(!/reload/i.test(help), 'release help must not advertise reload');
const rejected = spawnSync(binary, ['dev', '--help'], { encoding: 'utf8' });
assert.notEqual(rejected.status, 0, 'release must reject the dev command');
const bytes = await readFile(binary);
for (const marker of ['@vite/client', '__aggr/reload', 'AGGR_DEV_INTERNAL_WORKER', 'AGGR_DEV_CONTEXT']) {
  assert(!bytes.includes(Buffer.from(marker)), `release contains development code: ${marker}`);
}
console.log('Release omits the development command, launcher and live reload.');
