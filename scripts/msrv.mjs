import { readFile } from 'node:fs/promises';
import { spawn, spawnSync } from 'node:child_process';

const manifest = await readFile(new URL('../Cargo.toml', import.meta.url), 'utf8');
const version = /^rust-version = "(\d+\.\d+\.\d+)"$/m.exec(manifest)?.[1];
if (!version) throw new Error('Cargo.toml must declare the minimum Rust version');
const install = spawnSync('mise', ['install', `rust@${version}`], { stdio: 'inherit' });
if (install.error) throw install.error;
if (install.status !== 0) process.exit(install.status ?? 1);
const child = spawn('mise', ['exec', `rust@${version}`, '--', 'cargo', 'build', '--locked', ...process.argv.slice(2)], { stdio: 'inherit' });
child.on('error', error => { console.error(error.message); process.exitCode = 1; });
child.on('exit', code => { process.exitCode = code ?? 1; });
