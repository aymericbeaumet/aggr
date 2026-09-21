import { readFileSync, existsSync, readdirSync } from 'node:fs';
import { dirname, join, parse } from 'node:path';
import type { Plugin } from 'vite';

const licenseName = /^licen[cs]e(?:[.\-].*)?$/i;
// Code-unit order keeps client.LICENSE identical whatever order a filesystem lists a package's files in.
export function pickLicense(names: readonly string[]): string | undefined {
  const candidates = names.filter(name => licenseName.test(name)).sort();
  return candidates.find(name => name === 'LICENSE') ?? candidates[0];
}

export function licenses(): Plugin {
  const projectTerms = ['LICENSE', 'NOTICE', 'LICENSE-CONTRIBUTIONS'].map(file =>
    readFileSync(new URL(`../../${file}`, import.meta.url), 'utf8').trim(),
  );
  const banner = `/*! aggr — https://polyformproject.org/licenses/perimeter/1.0.1\n${projectTerms[1]}\n*/\n`;
  return {
    name: 'distribution-licenses',
    generateBundle: {
      order: 'post',
      handler(_options, bundle) {
        const packages = new Map<string, string>();
        for (const entry of Object.values(bundle)) {
          // Add the notice after minification so stripping ordinary comments cannot remove it.
          if (entry.type === 'asset' && entry.fileName.endsWith('.css')) {
            const source = typeof entry.source === 'string' ? entry.source : new TextDecoder().decode(entry.source);
            entry.source = banner + source;
          }
          if (entry.type !== 'chunk') continue;
          entry.code = banner + entry.code;
          for (const id of Object.keys(entry.modules)) {
            if (!id.includes('/node_modules/')) continue;
            let directory = dirname(id.split('?')[0]);
            while (directory !== parse(directory).root) {
              const file = join(directory, 'package.json');
              if (existsSync(file)) {
                const pkg = JSON.parse(readFileSync(file, 'utf8'));
                const key = `${pkg.name}@${pkg.version}`;
                if (!packages.has(key)) {
                  const license = pickLicense(readdirSync(directory));
                  if (!license) throw new Error(`Missing license for bundled dependency ${key}`);
                  packages.set(key, `${key}\n${readFileSync(join(directory, license), 'utf8').trim()}`);
                }
                break;
              }
              directory = dirname(directory);
            }
          }
        }
        const dependencyTerms = [...packages].sort(([a], [b]) => a.localeCompare(b)).map(([, license]) => license);
        this.emitFile({ type: 'asset', fileName: 'client.LICENSE', source: [...projectTerms, ...dependencyTerms].join('\n\n---\n\n') + '\n' });
      },
    },
  };
}
