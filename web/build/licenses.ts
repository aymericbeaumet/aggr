import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { dirname, join, parse } from 'node:path';
import type { Plugin } from 'vite';

/**
 * Emits `LICENSE` next to the bundle: the license text of every node_modules package whose
 * code ended up in an output chunk, so the committed assets carry their own attribution.
 */
export function licenses(fileName = 'LICENSE'): Plugin {
  return {
    name: 'dependency-licenses',
    apply: 'build',
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
              const pkg = JSON.parse(readFileSync(file, 'utf8')) as { name: string; version: string };
              const key = `${pkg.name}@${pkg.version}`;
              if (!packages.has(key)) {
                const license = readdirSync(directory).find((name) => /^licen[cs]e(?:\..*)?$/i.test(name));
                if (!license) throw new Error(`Missing license for bundled dependency ${key}`);
                packages.set(key, `${key}\n${readFileSync(join(directory, license), 'utf8').trim()}`);
              }
              break;
            }
            directory = dirname(directory);
          }
        }
      }
      const source = [...packages]
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([, license]) => license)
        .join('\n\n---\n\n');
      this.emitFile({ type: 'asset', fileName, source: `${source}\n` });
    },
  };
}
