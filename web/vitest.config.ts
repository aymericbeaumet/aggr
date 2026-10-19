import { svelte } from '@sveltejs/vite-plugin-svelte';
import { configDefaults, defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    projects: [
      {
        // Pure modules. The Svelte plugin compiles the `.svelte.ts` state modules they import.
        plugins: [svelte()],
        test: {
          name: 'unit',
          environment: 'node',
          include: ['src/**/*.test.ts'],
          exclude: [...configDefaults.exclude, 'src/**/*.dom.test.ts', 'src/sw/**'],
        },
      },
      {
        // Browser-side modules and the mounted app under jsdom. Svelte's client runtime is
        // behind the `browser` export condition.
        plugins: [svelte()],
        resolve: { conditions: ['browser'] },
        test: {
          name: 'dom',
          environment: 'jsdom',
          include: ['src/**/*.dom.test.ts'],
          exclude: [...configDefaults.exclude, 'src/sw/**'],
        },
      },
      {
        test: {
          name: 'sw',
          environment: 'node',
          include: ['src/sw/**/*.test.ts'],
        },
      },
    ],
  },
});
