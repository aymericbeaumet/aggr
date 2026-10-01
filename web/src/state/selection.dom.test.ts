import { describe, expect, it } from 'vitest';
import type { CursorRow } from '../selection/cursor';
import { selection } from './selection.svelte';

const row = (name: string): CursorRow => ({
  path: `items/blog/${name}`,
  href: `https://x.test/reads/items/blog/${name}/`,
  link: `https://blog.example/${name}`,
  title: name,
  original: null,
  discussions: [],
});

describe('selection', () => {
  it('selects the first row, steps, and remembers per list across syncs', () => {
    sessionStorage.clear();
    selection.configure('/reads/', sessionStorage);
    const page = 'https://x.test/reads/';
    selection.sync([row('a'), row('b'), row('c')], page);
    expect(selection.current?.title).toBe('a');
    expect(selection.move(1, page)?.title).toBe('b');
    expect(selection.move(1, page)?.title).toBe('c');
    expect(selection.move(1, page)?.title).toBe('c');
    expect(selection.edge('first', page)?.title).toBe('a');
    selection.move(1, page);
    // Another list has its own cursor; coming back restores this one.
    selection.sync([row('x'), row('y')], 'https://x.test/reads/page/2/');
    expect(selection.current?.title).toBe('x');
    selection.sync([row('a'), row('b'), row('c')], page);
    expect(selection.current?.title).toBe('b');
    // A remembered row that vanished gives way to the first.
    selection.sync([row('c')], page);
    expect(selection.current?.title).toBe('c');
    selection.sync([], page);
    expect(selection.current).toBeNull();
  });

  it('saves the cursor for the page being left', () => {
    sessionStorage.clear();
    selection.configure('/reads/', sessionStorage);
    const page = 'https://x.test/reads/sources/blog/';
    selection.sync([row('a'), row('b')], page);
    selection.url = row('b').href;
    selection.save(page);
    selection.sync([row('a'), row('b')], page);
    expect(selection.current?.title).toBe('b');
  });
});
