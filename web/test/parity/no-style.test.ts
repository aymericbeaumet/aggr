/// <reference types="node" />
import { readdirSync, readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

/**
 * `style.css` owns every class name the partials use, so a component that mirrors a partial
 * carries no `<style>` of its own. The check covers each fixture component and every
 * component it imports.
 */
const sources = import.meta.glob('../../src/**/*.svelte', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

const directory = new URL('./', import.meta.url);
const fixtureComponents = new Set(
  readdirSync(directory)
    .filter((file) => file.endsWith('.json'))
    .map((file) => (JSON.parse(readFileSync(new URL(file, directory), 'utf8')) as { component: string }).component),
);

function byName(name: string): [string, string] | undefined {
  return Object.entries(sources).find(([path]) => path.endsWith(`/${name}.svelte`));
}

/** The fixture components plus everything they import, transitively. */
function reachable(): Map<string, string> {
  const found = new Map<string, string>();
  const queue = [...fixtureComponents].flatMap((name) => {
    const source = byName(name);
    return source ? [source[0]] : [];
  });
  while (queue.length) {
    const path = queue.pop() as string;
    if (found.has(path)) continue;
    const source = sources[path];
    found.set(path, source);
    for (const match of source.matchAll(/from\s+'(\.[^']*?\/([\w-]+)\.svelte)'/g)) {
      const dependency = byName(match[2]);
      if (dependency && !found.has(dependency[0])) queue.push(dependency[0]);
    }
  }
  return found;
}

describe('parity components', () => {
  const found = reachable();

  it('cover every fixture component', () => {
    for (const name of fixtureComponents) {
      expect([...found.keys()].some((path) => path.endsWith(`/${name}.svelte`)), name).toBe(true);
    }
  });

  for (const [path, source] of found) {
    it(`${path.replace(/^.*\/src\//, 'src/')} has no <style>`, () => {
      expect(source).not.toMatch(/<style[\s>]/);
    });
  }
});
