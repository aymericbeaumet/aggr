import { valid, type PreferenceValue, type Schema } from './rules';

/**
 * Preferences on their way to another browser: a `{ version: 1, preferences: {…} }` envelope,
 * as a JSON file or base64url in the `#aggr-state=` fragment of a link, where it never reaches
 * a server log. Nothing else is accepted: no versionless payload, no storage-key export, and no
 * query string.
 */

export type Values = Record<string, PreferenceValue>;
export type Envelope = { version: 1; preferences: Values };

export const VERSION = 1;
/** The fragment parameter a shared link carries. */
export const PARAMETER = 'aggr-state';
/** A file or decoded link longer than this is refused unread. */
export const TEXT_LIMIT = 16_384;
/** The encoded fragment is bounded before it is decoded. */
const LINK_LIMIT = 22_000;

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value);

export function envelope(values: Values): Envelope {
  return { version: VERSION, preferences: { ...values } };
}

/** The settings an envelope carries. Throws when it is not one, or names an unknown or invalid setting. */
export function validate(schema: Schema, payload: unknown): Values {
  if (!isObject(payload)) throw new Error('Invalid preferences');
  if (payload.version !== VERSION || Object.keys(payload).some((key) => key !== 'version' && key !== 'preferences')) {
    throw new Error('Unsupported preferences version');
  }
  const source = payload.preferences;
  if (!isObject(source) || !Object.keys(source).length) throw new Error('Empty preferences');
  const values: Values = {};
  for (const [key, value] of Object.entries(source)) {
    const rule = Object.hasOwn(schema, key) ? schema[key] : undefined;
    if (!rule || !valid(rule, value)) throw new Error(`Invalid preference: ${key}`);
    values[key] = value;
  }
  return values;
}

/** The text of a file or of a decoded link, bounded, parsed and validated. */
export function parseTransfer(schema: Schema, raw: string): Values {
  if (raw.length > TEXT_LIMIT) throw new Error('Preferences file is too large');
  return validate(schema, JSON.parse(raw));
}

/** The file `Save file` writes. */
export function exportText(values: Values): string {
  return `${JSON.stringify(envelope(values), null, 2)}\n`;
}

/** The envelope as base64url, the alphabet a fragment carries untouched. */
export function encodeState(values: Values): string {
  return btoa(JSON.stringify(envelope(values))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** The JSON text a fragment carries. Throws when the fragment is not base64url. */
export function decodeState(encoded: string): string {
  if (encoded.length > LINK_LIMIT || !/^[A-Za-z0-9_-]+={0,2}$/.test(encoded)) throw new Error('Invalid preferences link');
  let base64 = encoded.replace(/-/g, '+').replace(/_/g, '/');
  while (base64.length % 4) base64 += '=';
  return atob(base64);
}

/** The link to `preferences/` under `root` that carries `values` in its fragment. */
export function shareLink(root: string, values: Values): string {
  const url = new URL('preferences/', root);
  url.hash = `${PARAMETER}=${encodeState(values)}`;
  return url.href;
}

/** The encoded payload of a fragment, or null when it carries none. A query string is never read. */
export function fragmentState(hash: string): string | null {
  return new URLSearchParams(hash.replace(/^#/, '')).get(PARAMETER);
}
