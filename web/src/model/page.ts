import type { ClientPage } from '../generated/ClientPage';
import { parse } from './parse';

const VIEWS = new Set(['list', 'article', 'preferences', 'offline', 'static']);

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value);

/** Whether a decoded value has the shape of the page model the client boots from. */
export function isClientPage(value: unknown): value is ClientPage {
  if (!isObject(value)) return false;
  if (!['base', 'path', 'kind', 'title'].every((key) => typeof value[key] === 'string')) return false;
  const { site, build, page } = value;
  if (!isObject(site) || !isObject(build) || !isObject(page)) return false;
  if (typeof build.app !== 'string' || typeof site.language !== 'string') return false;
  if (!isObject(site.assets) || !Array.isArray(site.entries) || !Array.isArray(site.discussions)) return false;
  return typeof page.view === 'string' && VIEWS.has(page.view);
}

/** The model a page embeds in `#aggr-page`, or null when it is missing or malformed. */
export function readModel(text: string | null | undefined): ClientPage | null {
  if (!text) return null;
  try {
    const value = parse(text);
    return isClientPage(value) ? value : null;
  } catch {
    return null;
  }
}
