/// <reference types="node" />
import { readdirSync, readFileSync } from 'node:fs';
import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';
import { components } from './components';
import { normalize } from './normalize';

/**
 * Every `<name>.json` here is `{ component, props }` written by the Rust parity test, next to
 * `<name>.html`, the markup the minijinja partial rendered for the same model. The Svelte
 * component must render the same thing.
 */
const directory = new URL('./', import.meta.url);
const fixtures = readdirSync(directory)
  .filter((file) => file.endsWith('.json'))
  .sort();

type Fixture = { component: string; props: Record<string, unknown> };

describe('parity', () => {
  it('has fixtures', () => {
    expect(fixtures.length).toBeGreaterThan(0);
  });

  for (const file of fixtures) {
    const name = file.slice(0, -'.json'.length);
    it(name, () => {
      const fixture = JSON.parse(readFileSync(new URL(file, directory), 'utf8')) as Fixture;
      const component = components[fixture.component];
      expect(component, `no component named ${fixture.component}`).toBeDefined();
      const expected = readFileSync(new URL(`${name}.html`, directory), 'utf8');
      const { body } = render(component, { props: fixture.props });
      expect(normalize(body)).toBe(normalize(expected));
    });
  }
});
