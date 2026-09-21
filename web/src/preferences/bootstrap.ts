import type { Preferences, PreferenceValue, PreferenceValues } from '../contracts';

type Rule = { initial: PreferenceValue; values?: PreferenceValue[]; min?: number; max?: number; attribute?: string };

export function createPreferences(defaults: Record<string, unknown>, stored: (key: string) => string | null): Preferences {
  const schema: Record<string, Rule> = {
    theme: { initial: 'auto', values: ['auto', 'light', 'dark', 'sepia'], attribute: 'theme' },
    'date-format': { initial: 'relative', values: ['relative', 'iso', 'local', 'local-time'], attribute: 'dateFormat' },
    'text-size': { initial: 'default', values: ['default', 'large', 'largest'], attribute: 'textSize' },
    'reading-width': { initial: 'standard', values: ['narrow', 'standard', 'wide'], attribute: 'readingWidth' },
    'font-family': { initial: 'sans', values: ['sans', 'serif', 'mono'], attribute: 'fontFamily' },
    'line-spacing': { initial: 'standard', values: ['compact', 'standard', 'relaxed'], attribute: 'lineSpacing' },
    'paragraph-spacing': { initial: 'standard', values: ['compact', 'standard', 'relaxed'], attribute: 'paragraphSpacing' },
    'paragraph-indent': { initial: false, values: [false, true], attribute: 'paragraphIndent' },
    'text-align': { initial: 'left', values: ['left', 'justify'], attribute: 'textAlign' },
    'letter-spacing': { initial: 'normal', values: ['normal', 'wide'], attribute: 'letterSpacing' },
    'word-spacing': { initial: 'normal', values: ['normal', 'wide'], attribute: 'wordSpacing' },
    'scroll-amount': { initial: 10, min: 1, max: 100 },
    'offline-items': { initial: 30, min: 0, max: 1000 },
    density: { initial: 'compact', values: ['compact', 'comfortable'], attribute: 'density' },
    thumbnails: { initial: 'show', values: ['show', 'hide'], attribute: 'thumbnails' },
    'feed-page-size': { initial: '50', values: ['10', '25', '50'] },
    motion: { initial: 'auto', values: ['auto', 'off'], attribute: 'motion' },
    'single-key-shortcuts': { initial: true, values: [true, false] },
  };

  function valid(key: string, value: unknown): value is PreferenceValue {
    if (!Object.prototype.hasOwnProperty.call(schema, key)) return false;
    const rule = schema[key];
    return rule.values ? rule.values.includes(value as PreferenceValue)
      : typeof value === 'number' && Number.isInteger(value) && value >= rule.min! && value <= rule.max!;
  }
  for (const [key, value] of Object.entries(defaults)) {
    if (valid(key, value)) schema[key].initial = value;
  }
  function storedValue(key: string, value: string | null) {
    if (typeof schema[key].initial === 'boolean') return value === 'true' ? true : value === 'false' ? false : null;
    if (typeof schema[key].initial === 'number') return value !== null && /^\d+$/.test(value) ? Number(value) : null;
    return value;
  }
  function read(): Preferences['values'] {
    const state: PreferenceValues = {};
    for (const key of Object.keys(schema)) {
      let raw = null;
      try { raw = stored('aggr:' + key); } catch { /* Storage can be unavailable in private browsing. */ }
      const value = storedValue(key, raw);
      state[key] = valid(key, value) ? value : schema[key].initial;
    }
    return state as Preferences['values'];
  }
  function validate(payload: unknown): PreferenceValues {
    if (!payload || typeof payload !== 'object' || Array.isArray(payload)) throw new Error('Invalid preferences');
    if (!('version' in payload) || payload.version !== 1 || Object.keys(payload).some(key => key !== 'version' && key !== 'preferences')) throw new Error('Unsupported preferences version');
    const source = 'preferences' in payload ? payload.preferences : null;
    if (!source || typeof source !== 'object' || Array.isArray(source) || !Object.keys(source).length) throw new Error('Empty preferences');
    const state: PreferenceValues = {};
    for (const [key, value] of Object.entries(source)) {
      if (!valid(key, value)) throw new Error('Invalid preference: ' + key);
      state[key] = value;
    }
    return state;
  }
  return { schema, values: read(), read, validate, valid };
}

export function applyInitialPreferences(preferences: Preferences, dataset: DOMStringMap) {
  for (const [key, rule] of Object.entries(preferences.schema)) {
    if (rule.attribute) dataset[rule.attribute] = String(preferences.values[key]);
  }
}
