import { svelte2tsx } from 'svelte2tsx';
declare global { var aggrProject: (json: string) => string; }
globalThis.aggrProject = (json: string): string => {
  const { source, filename } = JSON.parse(json);
  const result = svelte2tsx(source, { filename, isTsFile: true, mode: 'ts' });
  return JSON.stringify({ code: result.code, map: JSON.parse(result.map.toString()) });
};
