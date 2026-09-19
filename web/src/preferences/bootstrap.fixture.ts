import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import type { Preferences } from "../contracts";

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
  /** The site's configured defaults, rendered into the script as `site.preferences`. */
  defaults?: Record<string, unknown>;
}

// The template's bootstrap script is the schema's source of truth; tests run it as the browser would.
export function runBootstrap(options: BootstrapOptions = {}): BootstrapRun {
  const template = readFileSync(new URL("../../../themes/default/templates/base.html", import.meta.url), "utf8");
  const script = template.split('<script id="aggr-preferences">')[1].split("</script>")[0]
    .replace("{{ site.preferences | json }}", () => JSON.stringify(options.defaults ?? {}));
  const stored = options.stored ?? {};
  let writes = 0;
  const localStorage = {
    getItem: (key: string) => typeof stored === "function" ? stored() : stored[key] ?? null,
    setItem: () => { writes += 1; }
  };
  const dataset: Record<string, unknown> = {};
  const global: { AGGRPreferences?: Preferences } = {};
  runInNewContext(script, { window: global, document: { documentElement: { dataset } }, localStorage });
  return { preferences: global.AGGRPreferences!, dataset, writes: () => writes };
}

export function bootstrapSchema(): BootstrapSchema {
  return runBootstrap().preferences.schema as BootstrapSchema;
}
