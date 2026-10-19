import { isAbsolute, join } from 'node:path';

export function readContext(value = process.env.AGGR_DEV_CONTEXT) {
  if (!value) throw new Error('Start development with cargo run -- dev or make dev.');
  const context = JSON.parse(value);
  if (!context || !isAbsolute(context.repository ?? '') || !isAbsolute(context.cwd ?? '') ||
      !Array.isArray(context.workerArgs) || !context.workerArgs.every(argument => typeof argument === 'string') ||
      typeof context.viteOrigin !== 'string' ||
      (context.strictPort !== undefined && typeof context.strictPort !== 'boolean')) {
    throw new Error('Invalid development launcher context.');
  }
  return context;
}

export function commands(context) {
  const env = { AGGR_DEV_CONTEXT: JSON.stringify(context) };
  return [
    { name: 'rust', command: 'node dev/backend.mjs', env },
  ];
}

export function backendArguments(context) {
  return [
    '--restart', '--stop-signal', 'SIGINT', '--stop-timeout', '10s', '--shell=none',
    '--project-origin', context.repository,
    '--watch', join(context.repository, 'src'),
    '--watch-non-recursive', context.repository,
    '--filter', '*.rs', '--filter', 'Cargo.toml', '--filter', 'Cargo.lock',
    '--ignore', '/src/theme/**',
    '--', 'cargo', 'run', '--locked', '--manifest-path', join(context.repository, 'Cargo.toml'),
    '--', ...context.workerArgs,
  ];
}
