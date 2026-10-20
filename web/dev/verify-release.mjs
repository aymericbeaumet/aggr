import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const filename = process.platform === 'win32' ? 'aggr.exe' : 'aggr';
const binary = resolve(process.argv[2] ?? fileURLToPath(new URL(`../../target/release/${filename}`, import.meta.url)));
for (const args of [['--help'], ['dev', '--help']]) {
  const result = spawnSync(binary, args, { encoding: 'utf8' });
  assert.equal(result.status, 0, result.error?.message ?? result.stderr);
  if (args[0] === 'dev') {
    for (const flag of ['--native', '--demo', '--release']) assert(result.stdout.includes(flag));
  } else assert.match(result.stdout, /^\s+dev\s/m);
}
console.log('Release exposes native development, isolated demo, and publication preview.');
