import type { BootstrapRule } from '../generated/BootstrapRule';
import type { JsonValue } from '../generated/serde_json/JsonValue';

export type PreferenceValue = string | number | boolean;
/** The validation rules `#aggr-preferences` embeds, keyed by setting name. */
export type Schema = Record<string, BootstrapRule>;

/** The rules a page embeds; an absent or malformed block means no setting is known. */
export function parseSchema(text: string | null | undefined): Schema {
  if (!text) return {};
  try {
    const value: unknown = JSON.parse(text);
    return typeof value === 'object' && value !== null && !Array.isArray(value) ? (value as Schema) : {};
  } catch {
    return {};
  }
}

/** Whether `value` is one the rule allows: a listed choice, or an integer within the bounds. */
export function valid(rule: BootstrapRule, value: unknown): value is PreferenceValue {
  if (rule.values) return rule.values.includes(value as JsonValue);
  return (
    typeof value === 'number' &&
    Number.isInteger(value) &&
    value >= (rule.min ?? Number.NEGATIVE_INFINITY) &&
    value <= (rule.max ?? Number.POSITIVE_INFINITY)
  );
}

/** Storage holds strings; restore the type the rule's initial value declares before validating. */
export function coerce(rule: BootstrapRule, raw: string | null): unknown {
  if (raw === null) return null;
  if (typeof rule.initial === 'boolean') return raw === 'true' ? true : raw === 'false' ? false : null;
  if (typeof rule.initial === 'number') return /^\d+$/.test(raw) ? Number(raw) : null;
  return raw;
}

/** Every setting's current value: the stored one when it is valid, else the rule's initial. */
export function readPreferences(schema: Schema, get: (key: string) => string | null): Record<string, PreferenceValue> {
  const values: Record<string, PreferenceValue> = {};
  for (const [key, rule] of Object.entries(schema)) {
    let raw: string | null = null;
    try {
      raw = get(key);
    } catch {
      raw = null; // private browsing
    }
    const value = coerce(rule, raw);
    values[key] = valid(rule, value) ? value : (rule.initial as PreferenceValue);
  }
  return values;
}

/** The `localStorage` key of a setting. */
export const storageKey = (key: string): string => `aggr:${key}`;

export function positiveInteger(value: unknown, fallback: number): number {
  const parsed = Number.parseInt(String(value), 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

/** Every setting at the initial value the build declared. */
export function defaults(schema: Schema): Record<string, PreferenceValue> {
  return Object.fromEntries(Object.entries(schema).map(([key, rule]) => [key, rule.initial as PreferenceValue]));
}

/** Write each setting that drives CSS onto `root.dataset`, as the stylesheet reads it. */
export function applyDataset(schema: Schema, values: Record<string, PreferenceValue>, root: HTMLElement): void {
  for (const [key, rule] of Object.entries(schema)) {
    if (rule.attribute && key in values) root.dataset[rule.attribute] = String(values[key]);
  }
}
