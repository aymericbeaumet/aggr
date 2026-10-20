import { spawnSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const names = ['aggr-ui-model', 'aggr-lib', 'aggr'];

function packages(metadata, version, ref) {
  const selected = names.map(name => {
    const pkg = metadata.packages.find(pkg => pkg.name === name);
    if (!pkg) throw new Error(`Missing workspace package ${name}`);
    return { name, version: pkg.version };
  });
  if (metadata.packages.find(pkg => pkg.name === 'aggr')?.version !== version || selected[1].version !== version)
    throw new Error('VERSION, aggr and aggr-lib versions must agree');
  if (ref.type === 'tag' && ref.name !== `v${version}`)
    throw new Error(`Tag ${ref.name} does not match v${version}`);
  return selected;
}

async function published(pkg, fetchIndex) {
  const { name, version } = pkg;
  const url = `https://index.crates.io/${name.slice(0, 2)}/${name.slice(2, 4)}/${name}`;
  const response = await fetchIndex(url, { cache: 'no-store', signal: AbortSignal.timeout(15_000) });
  if (response.status === 404) return false;
  if (!response.ok) throw new Error(`Checking ${name} in crates.io failed (HTTP ${response.status})`);
  const text = (await response.text()).trim();
  if (!text) throw new Error(`Empty registry index for ${name}`);
  const rows = text.split('\n').map(line => JSON.parse(line));
  if (rows.some(row => row.name !== name || typeof row.vers !== 'string' || typeof row.yanked !== 'boolean'))
    throw new Error(`Invalid registry index for ${name}`);
  const found = rows.find(row => row.vers === version);
  if (found?.yanked) throw new Error(`${name} ${version} is yanked; choose a new version before publishing`);
  return found !== undefined;
}

function buildEnvironment(env) {
  const result = { ...env };
  delete result.CARGO_REGISTRY_TOKEN;
  delete result.CARGO_REGISTRIES_CRATES_IO_TOKEN;
  return result;
}

export async function publishPackages({
  metadata, version, ref = {}, args = [], env = process.env,
  fetchIndex = fetch, run, log = console.log,
}) {
  if (args.some(arg => arg !== '--allow-dirty'))
    throw new Error('Publishing accepts only --allow-dirty');
  const targets = packages(metadata, version.trim(), ref);
  const existing = await Promise.all(targets.map(pkg => published(pkg, fetchIndex)));
  const pending = targets.filter((pkg, index) => {
    if (existing[index]) log(`${pkg.name} ${pkg.version} already published; skipping`);
    return !existing[index];
  });
  if (!pending.length) {
    log('Nothing to publish; all three versions already exist in crates.io.');
    return;
  }
  if (env.GITHUB_ACTIONS === 'true' && !env.CARGO_REGISTRY_TOKEN)
    throw new Error('Add the CARGO_REGISTRY_TOKEN repository Actions secret to publish new crate versions');
  await run('mise', ['run', 'package', '--', ...args], { env: buildEnvironment(env) });
  await run('cargo', [
    'publish', '--registry', 'crates-io',
    ...pending.flatMap(pkg => ['-p', `${pkg.name}@${pkg.version}`]), ...args,
  ], { env });
}

function command(program, args, options = {}) {
  const result = spawnSync(program, args, { cwd: root, stdio: 'inherit', ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${program} ${args.join(' ')} failed (${result.status ?? result.signal})`);
}

if (import.meta.main) {
  try {
    const result = spawnSync('cargo', ['metadata', '--no-deps', '--format-version', '1', '--locked'], {
      cwd: root, env: buildEnvironment(process.env), encoding: 'utf8', maxBuffer: 8 * 1024 * 1024,
    });
    if (result.error) throw result.error;
    if (result.status !== 0) throw new Error(`Reading Cargo metadata failed: ${result.stderr.trim()}`);
    await publishPackages({
      metadata: JSON.parse(result.stdout), version: await readFile(new URL('../VERSION', import.meta.url), 'utf8'),
      ref: { type: process.env.GITHUB_REF_TYPE, name: process.env.GITHUB_REF_NAME },
      args: process.argv.slice(2), run: command,
    });
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
