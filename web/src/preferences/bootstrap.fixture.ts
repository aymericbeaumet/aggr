import type { Preferences } from "../contracts";
import { applyInitialPreferences, createPreferences } from './bootstrap';

export type BootstrapSchema = Record<string, { initial: unknown; values?: unknown[]; min?: number; max?: number; attribute?: string }>;

export interface BootstrapRun {
  preferences: Preferences;
  /** The `data-*` attributes the script wrote on `<html>`, as the fake element received them. */
  dataset: Record<string, unknown>;
  /** How many times the script tried to write storage: it never should. */
  writes(): number;
}

export interface BootstrapOptions {
  /** Stored `aggr:*` values, or a getter that throws when storage is unavailable. */
  stored?: Record<string, string> | (() => never);
  /** The site's configured defaults, supplied by its JSON data block. */
  defaults?: Record<string, unknown>;
}

export function runBootstrap(options: BootstrapOptions = {}): BootstrapRun {
  const stored = options.stored ?? {};
  let writes = 0;
  const localStorage = {
    getItem: (key: string) => typeof stored === "function" ? stored() : stored[key] ?? null,
    setItem: () => { writes += 1; }
  };
  const dataset: DOMStringMap = {};
  const preferences = createPreferences(options.defaults ?? {}, localStorage.getItem);
  applyInitialPreferences(preferences, dataset);
  return { preferences, dataset, writes: () => writes };
}

export function bootstrapSchema(): BootstrapSchema {
  return runBootstrap().preferences.schema as BootstrapSchema;
}
