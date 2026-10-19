import { fileURLToPath, URL } from 'node:url';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';

export const modelPath = fileURLToPath(new URL('./.generated/types', import.meta.url));
export default defineConfig({
  base: './',
  plugins: [svelte()],
  resolve: { alias: { '@aggr-model': modelPath } },
  ssr: { noExternal: true },
  server: { strictPort: true, host: '127.0.0.1' },
});
