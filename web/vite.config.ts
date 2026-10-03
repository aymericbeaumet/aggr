import { fileURLToPath, URL } from 'node:url';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';
import { licenses } from './build/licenses.ts';

// Compiled output is committed under the embedded theme; the Rust binary serves it as-is.
export const outDir = fileURLToPath(new URL('../themes/default/static/app', import.meta.url));

// Localhost origins only: the dev server is reached from aggr's own `dev` pages.
export const localhostOrigins = /^https?:\/\/(localhost|127\.0\.0\.1|\[::1\])(?::\d+)?$/;

export default defineConfig({
  base: './',
  plugins: [svelte(), licenses()],
  server: { strictPort: true, cors: { origin: localhostOrigins } },
  build: {
    outDir,
    emptyOutDir: true,
    manifest: true,
    cssCodeSplit: false,
    target: 'es2022',
    rollupOptions: {
      input: fileURLToPath(new URL('./src/main.ts', import.meta.url)),
      output: {
        entryFileNames: 'app-[hash].js',
        chunkFileNames: '[name]-[hash].js',
        assetFileNames: 'app-[hash][extname]',
      },
    },
  },
});
