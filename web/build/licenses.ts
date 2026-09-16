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
  return {
    name: 'dependency-licenses',
    generateBundle(_options, bundle) {
      const packages = new Map<string, string>();
      for (const entry of Object.values(bundle)) {
        if (entry.type !== 'chunk') continue;
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
      this.emitFile({ type: 'asset', fileName: 'client.LICENSE', source: [...packages].sort(([a], [b]) => a.localeCompare(b)).map(([, license]) => license).join('\n\n---\n\n') + '\n' });
    },
  };
}
