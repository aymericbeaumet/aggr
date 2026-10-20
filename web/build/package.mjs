import { cp, mkdir, rm } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const root = fileURLToPath(new URL('../..', import.meta.url));
const destination = join(root, 'crates/aggr-lib/.generated/publish');
const staged = `${destination}.staging`;
await rm(staged, { recursive: true, force: true });
await mkdir(staged, { recursive: true });
const result = spawnSync(process.execPath, [join(root, 'web/build.ts')], {
  cwd: join(root, 'web'), env: { ...process.env, AGGR_UI_OUT_DIR: join(staged, 'ui') }, stdio: 'inherit',
});
if (result.status !== 0) throw new Error('Could not prepare the packaged reader. Run make build first.');
for (const [source, name] of [
  ['config.default.toml', 'config.default.toml'], ['examples/starter.toml', 'starter.toml'],
  ['examples/aggr.toml', 'example.toml'], ['docs/aggr-instance.schema.json', 'aggr-instance.schema.json'],
  ['Cargo.lock', 'Cargo.lock'],
]) await cp(join(root, source), join(staged, name));
await mkdir(dirname(destination), { recursive: true });
await rm(destination, { recursive: true, force: true });
await cp(staged, destination, { recursive: true });
await rm(staged, { recursive: true, force: true });
console.log(`Prepared self-contained library inputs in ${resolve(destination)}`);
