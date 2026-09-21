import { fileURLToPath } from 'node:url';
import { build, type Plugin } from 'vite';
import { licenses } from './licenses.ts';

export function bootstrap(): Plugin {
  return {
    name: 'prepaint-bootstrap',
    apply: 'build',
    async generateBundle() {
      const result = await build({
        configFile: false,
        logLevel: 'silent',
        plugins: [licenses()],
        build: {
          write: false,
          minify: 'terser',
          terserOptions: { format: { comments: false } },
          lib: {
            entry: fileURLToPath(new URL('../src/bootstrap.ts', import.meta.url)),
            name: 'AggrBootstrap',
            formats: ['iife'],
            fileName: () => 'bootstrap.js',
          },
        },
      });
      for (const bundle of Array.isArray(result) ? result : [result]) {
        if (!('output' in bundle)) continue;
        for (const entry of bundle.output) {
          if (entry.type !== 'chunk') continue;
          for (const module of entry.moduleIds) this.addWatchFile(module);
          this.emitFile({ type: 'asset', fileName: entry.fileName, source: entry.code });
        }
      }
    },
  };
}
