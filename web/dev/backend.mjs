import { execa } from 'execa';
import { backendArguments, readContext } from './context.mjs';

const context = readContext();
const result = await execa('watchexec', backendArguments(context), {
  cwd: context.cwd,
  env: {
    AGGR_DEV_INTERNAL_WORKER: '1',
    AGGR_VITE_URL: context.viteOrigin,
  },
  stdio: 'inherit',
  reject: false,
});
process.exitCode = result.exitCode ?? 1;
