import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { build } from 'vite';
import { describe, expect, it } from 'vitest';
import { licenses, pickLicense } from './licenses.ts';

describe('bundled dependency licence files', () => {
  it('picks the same file whatever order the filesystem lists entries in', () => {
    const names = ['README.md', 'package.json', 'license.txt', 'LICENSE.md', 'LICENSE', 'LICENCE'];
    expect(pickLicense(names)).toBe('LICENSE');
    expect(pickLicense([...names].reverse())).toBe('LICENSE');
    expect(pickLicense([...names].sort(() => -1))).toBe('LICENSE');
    expect(pickLicense(['README.md', 'licence.txt', 'LICENSE.md'])).toBe('LICENSE.md');
    expect(pickLicense(['LICENSE.md', 'licence.txt', 'README.md'])).toBe('LICENSE.md');
  });
  it('accepts LICENSE-MIT style names and ignores unrelated files', () => {
    expect(pickLicense(['LICENSE-MIT', 'LICENSE-APACHE', 'index.js'])).toBe('LICENSE-APACHE');
    expect(pickLicense(['LICENSE-APACHE', 'LICENSE-MIT'])).toBe('LICENSE-APACHE');
    expect(pickLicense(['license-mit.txt'])).toBe('license-mit.txt');
    expect(pickLicense(['index.js', 'LICENSES.txt', 'unlicensed', 'licenser'])).toBeUndefined();
    expect(pickLicense([])).toBeUndefined();
  });
});

it('ships project terms and preserves the required notice in minified JS and CSS', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'aggr-license-build-'));
  try {
    writeFileSync(join(directory, 'entry.js'), 'import "./style.css"; console.log("reader");');
    writeFileSync(join(directory, 'style.css'), 'body { color: red; }');
    const result = await build({
      configFile: false,
      logLevel: 'silent',
      plugins: [licenses()],
      build: {
        write: false,
        minify: 'terser',
        terserOptions: { format: { comments: false } },
        lib: { entry: join(directory, 'entry.js'), name: 'Reader', formats: ['iife'] },
      },
    });
    const output = (Array.isArray(result) ? result : [result]).flatMap(result => 'output' in result ? result.output : []);
    const notice = readFileSync(new URL('../../NOTICE', import.meta.url), 'utf8').trim();
    for (const extension of ['.js', '.css']) {
      const asset = output.find(entry => entry.fileName.endsWith(extension));
      expect(asset).toBeDefined();
      const content = asset!.type === 'chunk' ? asset!.code : String(asset!.source);
      expect(content).toContain(notice);
      expect(content).toContain('https://polyformproject.org/licenses/perimeter/1.0.1');
    }
    const terms = output.find(entry => entry.fileName === 'client.LICENSE');
    expect(terms?.type).toBe('asset');
    for (const file of ['LICENSE', 'NOTICE', 'LICENSE-CONTRIBUTIONS']) {
      expect(terms?.type === 'asset' ? String(terms.source) : '').toContain(
        readFileSync(new URL(`../../${file}`, import.meta.url), 'utf8').trim(),
      );
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
