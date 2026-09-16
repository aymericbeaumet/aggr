import { readFileSync } from 'node:fs';
import { compile } from 'svelte/compiler';
import { describe, expect, it } from 'vitest';

// bits-ui re-registers a Command.Item whenever its `value` prop is dirtied and re-selects the first entry after
// the tick. Svelte keys each-block items with `safe_equals`, so any reconcile re-sets every item signal: the
// menu must therefore iterate a derived of the suggestions (unrelated store emissions never reconcile) and hand
// each item a derived id (a refreshed list with the same ids never dirties `value`). This pins the compiled shape.
describe('completion menu compilation', () => {
  const source = readFileSync(new URL('./Controls.svelte', import.meta.url), 'utf8');
  const { js } = compile(source, { filename: 'Controls.svelte', generate: 'client', runes: true, dev: false });

  it('iterates a derived suggestions list rather than the whole model store', () => {
    expect(js.code).toMatch(/\$\.each\([^,]+,\s*\d+,\s*\(\)\s*=>\s*\$\.get\(suggestions\)/);
    expect(js.code).not.toMatch(/\$\.each\([^,]+,\s*\d+,\s*\(\)\s*=>\s*\$model\(\)/);
  });

  it('passes each Command.Item a derived id instead of reading the item object', () => {
    const items = js.code.slice(js.code.indexOf('Command.Item'));
    expect(items).not.toBe('');
    expect(items).toMatch(/get value\(\)\s*\{\s*return \$\.get\(id\);/);
    expect(items).not.toMatch(/get value\(\)\s*\{\s*return \$\.get\(suggestion\)\.id/);
  });
});
