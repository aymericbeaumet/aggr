import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

export default {
  preprocess: vitePreprocess(),
  // Runes only; component styles are extracted by Vite into the single app-[hash].css bundle.
  compilerOptions: { runes: true, css: 'external' },
};
