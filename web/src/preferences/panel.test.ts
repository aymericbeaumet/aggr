import { afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'svelte/server';
import Panel from './Panel.svelte';
import type { PreferencesService } from './service';
import type { PreferenceActions } from './actions';
import { bootstrapSchema } from './bootstrap.fixture';

function html() {
  vi.stubGlobal('window', {});
  const values = Object.fromEntries(Object.entries(bootstrapSchema()).map(([key, rule]) => [key, rule.initial as string]));
  const service = { getSnapshot: () => ({ values, pending: null, status: '', link: '', offlineSummary: '' }), subscribe: () => () => {} } as unknown as PreferencesService;
  const actions = { shortcutURL: '/preferences/#shortcut-help', canShare: false, run: vi.fn() } as unknown as PreferenceActions;
  return render(Panel, { props: { service, actions, onShortcuts: () => {} } }).body;
}
afterEach(() => vi.unstubAllGlobals());

describe('preferences panel', () => {
  it('renders one control per schema key, selects carrying every schema value', () => {
    const schema = bootstrapSchema();
    const body = html();
    const controls = [...body.matchAll(/<(select|input) id="([^"]+)" data-preference="([^"]+)"/g)].map(([, tag, id, key]) => ({ tag, id, key }));
    expect(controls.map(control => control.key).sort()).toEqual(Object.keys(schema).sort());
    expect(controls.find(control => control.key === 'theme')?.id).toBe('theme-mode');
    for (const control of controls) if (control.key !== 'theme') expect(control.id).toBe(control.key);
    const selects = [...body.matchAll(/<select id="[^"]+" data-preference="([^"]+)"[^>]*>(.*?)<\/select>/g)]
      .map(([, key, options]) => [key, [...options.matchAll(/<option value="([^"]*)"/g)].map(match => match[1])] as const);
    expect(selects.length).toBe(14);
    for (const [key, values] of selects) expect(values, key).toEqual(schema[key].values?.map(String));
  });
});
