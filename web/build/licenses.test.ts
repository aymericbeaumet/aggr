import { describe, expect, it } from 'vitest';
import { pickLicense } from './licenses.ts';

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
