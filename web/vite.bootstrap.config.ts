import { fileURLToPath, URL } from 'node:url';
import { defineConfig } from 'vite';
import { outDir } from './vite.config.ts';

// The inline bootstrap is a classic script that runs before the app module; an IIFE cannot
// share the code-split app build, hence its own single-file build into the same directory.
export default defineConfig({
  build: {
    outDir,
    emptyOutDir: false,
    target: 'es2022',
    lib: {
      entry: fileURLToPath(new URL('./src/bootstrap.ts', import.meta.url)),
      name: 'AggrBootstrap',
      formats: ['iife'],
      fileName: () => 'bootstrap.js',
    },
  },
});
