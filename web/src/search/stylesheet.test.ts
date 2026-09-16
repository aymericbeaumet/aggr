import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

// The mounted search field replaces the server-rendered one and must render pixel-identically (the browser
// contract search_control_mount_and_delayed_facets samples both at 1280 and 390 px). The two are kept identical
// by construction: every rule that styles the mounted field lists the static field in the same selector list,
// so there is no second copy of the geometry that could be edited on its own.
const css = readFileSync(new URL('../../../themes/default/static/style.css', import.meta.url), 'utf8');
const source = readFileSync(new URL('./Controls.svelte', import.meta.url), 'utf8');

const STATIC_FIELD = '.feed-search input[type="search"]';
const MOUNTED_FIELD = '.search-input-line #q';

type Rule = { selectors: string[]; declarations: Map<string, string> };

/** Innermost `selectors { declarations }` rules, whether top-level or nested in an at-rule. */
function styleRules(stylesheet: string): Rule[] {
  const rules: Rule[] = [];
  const text = stylesheet.replace(/\/\*[\s\S]*?\*\//g, '');
  for (const [, prelude, body] of text.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selectors = prelude.split(',').map((selector) => selector.trim()).filter(Boolean);
    const declarations = new Map<string, string>();
    for (const declaration of body.split(';')) {
      const colon = declaration.indexOf(':');
      if (colon > 0) declarations.set(declaration.slice(0, colon).trim(), declaration.slice(colon + 1).trim());
    }
    rules.push({ selectors, declarations });
  }
  return rules;
}

describe('search field stylesheet contract', () => {
  const rules = styleRules(css);
  const mounted = rules.filter((rule) => rule.selectors.includes(MOUNTED_FIELD));

  it('styles the mounted field only through selector lists it shares with the static field', () => {
    expect(mounted.length).toBeGreaterThan(0);
    for (const rule of mounted) expect(rule.selectors).toContain(STATIC_FIELD);
  });

  it('declares the field geometry once for both fields', () => {
    const geometry = rules.filter((rule) => rule.selectors.includes(STATIC_FIELD) && rule.declarations.has('min-height'));
    expect(geometry).toHaveLength(1);
    const [rule] = geometry;
    expect(rule.selectors).toEqual([STATIC_FIELD, MOUNTED_FIELD]);
    expect([...rule.declarations.keys()].sort()).toEqual(
      ['background', 'border', 'border-radius', 'box-sizing', 'color', 'font', 'min-height', 'min-width', 'padding', 'width'].sort()
    );
    expect(rule.declarations.get('min-height')).toBe('44px');
    expect(rule.declarations.get('padding')).toBe('.55rem 2.75rem .55rem .7rem');
  });

  it('keeps the high-contrast borders next to the field rules for both fields', () => {
    const borders = mounted.filter((rule) => rule.declarations.has('border-color') && rule.declarations.size === 1);
    expect(borders.map((rule) => rule.declarations.get('border-color'))).toEqual(['var(--control-border)', 'ButtonBorder']);
  });

  it('leaves no component stylesheet that could shadow the shared rules', () => {
    expect(source).not.toMatch(/<style[\s>]/);
    for (const name of ['search-control', 'search-command', 'search-input-line', 'search-clear', 'search-completions', 'search-completion', 'completion-label', 'search-query-help', 'search-error', 'search-offline']) {
      expect(source).toContain(name);
      expect(rules.some((rule) => rule.selectors.some((selector) => selector.includes(`.${name}`)))).toBe(true);
    }
  });
});
