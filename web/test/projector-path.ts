const normalize = (value: string): string => {
  const absolute = value.startsWith('/');
  const parts: string[] = [];
  for (const part of value.replaceAll('\\', '/').split('/')) {
    if (part === '..') {
      if (parts.length && parts.at(-1) !== '..') parts.pop();
      else if (!absolute) parts.push(part);
    } else if (part && part !== '.') parts.push(part);
  }
  return (absolute ? '/' : '') + parts.join('/') || '.';
};
export const join = (...parts: string[]): string => normalize(parts.join('/'));
export const resolve = (...parts: string[]): string => join(...parts);
export const dirname = (value: string): string => {
  const normalized = normalize(value);
  const slash = normalized.lastIndexOf('/');
  return slash < 0 ? '.' : normalized.slice(0, slash) || '/';
};
export const basename = (value: string): string => normalize(value).split('/').at(-1) ?? '';
export const parse = (value: string) => {
  const base = basename(value);
  const dot = base.lastIndexOf('.');
  const ext = dot > 0 ? base.slice(dot) : '';
  return { root: value.startsWith('/') ? '/' : '', dir: dirname(value), base, ext, name: ext ? base.slice(0, -ext.length) : base };
};
export default { join, resolve, dirname, basename, parse };
