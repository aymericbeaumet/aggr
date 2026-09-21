import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { runInNewContext } from 'node:vm';
import { build } from 'vite';
import { describe, expect, it, vi } from 'vitest';
import { bootstrap } from './bootstrap.ts';

describe('prepaint bootstrap delivery', () => {
  it('loads one reusable classic script before styles or page content', () => {
    const template = readFileSync(new URL('../../themes/default/templates/base.html', import.meta.url), 'utf8');
    const script = template.match(/<script[^>]+src="[^\n]*assets\/bootstrap\.js[^\n]*<\/script>/)?.[0];
    expect(script).toBeDefined();
    expect(script).not.toMatch(/\b(?:async|defer|type="module")\b/);
    expect(template.indexOf(script!)).toBeLessThan(template.indexOf('rel="stylesheet"'));
    expect(template).toContain('<script id="aggr-preferences" type="application/json">{{ site.preferences | json }}</script>');
    expect(template).not.toContain('window.AGGRPreferences =');
    expect(template).not.toContain('{% include "_dates.html" %}');
  });

  it('runs preferences synchronously and formats parsed dates before DOMContentLoaded', async () => {
    const directory = mkdtempSync(join(tmpdir(), 'aggr-bootstrap-'));
    try {
      writeFileSync(join(directory, 'entry.js'), 'console.log("reader");');
      const result = await build({
        configFile: false,
        logLevel: 'silent',
        plugins: [bootstrap()],
        build: { write: false, lib: { entry: join(directory, 'entry.js'), name: 'Reader', formats: ['iife'] } },
      });
      const output = (Array.isArray(result) ? result : [result]).flatMap(result => 'output' in result ? result.output : []);
      const script = output.find(entry => entry.fileName === 'bootstrap.js');
      expect(script?.type).toBe('asset');
      const code = script?.type === 'asset' ? String(script.source) : '';
      expect(code).toContain('Required Notice:');
      const dataset: Record<string, string> = {};
      const documentEvents = new Map<string, () => void>();
      let changes!: (records: unknown[]) => void;
      const disconnect = vi.fn();
      const observe = vi.fn();
      const document = {
        readyState: 'loading',
        documentElement: { dataset },
        getElementById: () => ({ textContent: JSON.stringify({ theme: 'sepia', 'date-format': 'iso' }) }),
        querySelectorAll: () => [],
        addEventListener: (event: string, callback: () => void) => documentEvents.set(event, callback),
      };
      const window: { AGGRPreferences?: { values: Record<string, unknown> }; AGGRDates?: { text(timestamp: number, format: string): string | null } } = {};
      runInNewContext(code, {
        window, document,
        localStorage: { getItem: (key: string) => key === 'aggr:theme' ? 'dark' : null },
        MutationObserver: class {
          constructor(callback: typeof changes) { changes = callback; }
          observe = observe;
          disconnect = disconnect;
          takeRecords = () => [];
        },
      });
      expect(dataset.theme).toBe('dark');
      expect(dataset.dateFormat).toBe('iso');
      expect(window.AGGRPreferences?.values.theme).toBe('dark');
      expect(observe).toHaveBeenCalledOnce();
      const time = {
        nodeType: 1, textContent: 'original date',
        getAttribute: () => '2026-09-20T12:30:00Z',
        closest: (): unknown => time,
      };
      changes([{ target: time, addedNodes: [] }]);
      expect(time.textContent).toBe('2026-09-20');
      expect(disconnect).not.toHaveBeenCalled();
      expect(window.AGGRDates?.text(Infinity, 'iso')).toBeNull();
      documentEvents.get('DOMContentLoaded')!();
      expect(disconnect).toHaveBeenCalledOnce();
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
});
