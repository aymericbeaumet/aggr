import { isAbsolute, join, relative } from 'node:path';

export function readContext(value = process.env.AGGR_DEV_CONTEXT) {
  if (!value) throw new Error('Start development with cargo run -- dev or make dev.');
  const context = JSON.parse(value);
  if (!context || !isAbsolute(context.repository ?? '') || !isAbsolute(context.cwd ?? '') ||
      !isAbsolute(context.executable ?? '') || !Array.isArray(context.workerArgs) ||
      !context.workerArgs.every(argument => typeof argument === 'string') ||
      typeof context.viteOrigin !== 'string' ||
      (context.strictPort !== undefined && typeof context.strictPort !== 'boolean')) {
    throw new Error('Invalid development launcher context.');
  }
  return context;
}

export function buildArguments(context) {
  return [
    'build', '--locked', '--manifest-path', join(context.repository, 'Cargo.toml'),
    '--bin', 'aggr-dev-worker', '--features', 'dev-worker',
    '--message-format=json-render-diagnostics',
  ];
}

export function workerArguments(context, restart) {
  const result = [];
  for (let index = 0; index < context.workerArgs.length; index++) {
    const argument = context.workerArgs[index];
    if (argument === '--port' || argument === '-p') { index++; continue; }
    if (argument.startsWith('--port=') || /^-p\d/.test(argument)) continue;
    if (restart && argument === '--clean') continue;
    result.push(argument);
  }
  return [...result, '--port', '0'];
}

export function needsBuild(context, file) {
  const path = relative(context.repository, file).replaceAll('\\', '/');
  if (path.startsWith('../') || path.startsWith('target/') || path.includes('/node_modules/')) return false;
  if (['Cargo.toml', 'Cargo.lock', 'build.rs', '.cargo/config.toml'].includes(path)) return true;
  if ((path.startsWith('src/') || path.startsWith('crates/')) && /(?:\.rs|Cargo\.toml)$/.test(path)) return true;
  return context.workerArgs.includes('--release') &&
    /^(?:web\/src\/|web\/public\/|web\/build|web\/vite|web\/package)/.test(path);
}
