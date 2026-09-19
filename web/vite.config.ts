import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath, URL } from 'node:url';
import { licenses } from './build/licenses.ts';

export default defineConfig({
  plugins: [svelte(), licenses()],
  test: { include: ['src/**/*.test.ts', 'build/**/*.test.ts'], environment: 'node' },
  server: { strictPort: true, cors: { origin: /^http:\/\/(localhost|127\.0\.0\.1|\[::1\])(?::\d+)?$/ } },
  build: {
    outDir: fileURLToPath(new URL('../themes/default/static', import.meta.url)),
    emptyOutDir: false,
    cssCodeSplit: false,
    minify: 'terser',
    terserOptions: { format: { comments: false } },
    lib: {
      entry: fileURLToPath(new URL('./src/main.ts', import.meta.url)),
      name: 'AggrReader',
      formats: ['iife'],
      fileName: () => 'app.js',
      cssFileName: 'client',
    },
  },
});
