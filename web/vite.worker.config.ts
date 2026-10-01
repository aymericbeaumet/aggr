import { fileURLToPath, URL } from 'node:url';
import { defineConfig } from 'vite';
import { outDir } from './vite.config.ts';

// The service worker is registered as a classic worker script, so it is bundled alone as an
// IIFE next to the app build.
export default defineConfig({
  build: {
    outDir,
    emptyOutDir: false,
    target: 'es2022',
    lib: {
      entry: fileURLToPath(new URL('./src/sw/main.ts', import.meta.url)),
      name: 'AggrWorker',
      formats: ['iife'],
      fileName: () => 'worker.js',
    },
  },
});
