import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import type { Preferences } from "../contracts";

export type BootstrapSchema = Record<string, { initial: unknown; values?: unknown[]; min?: number; max?: number }>;

// The template's bootstrap script is the schema's source of truth; tests run it as the browser would.
export function bootstrapSchema(): BootstrapSchema {
  const template = readFileSync(new URL("../../../themes/default/templates/base.html", import.meta.url), "utf8");
  const script = template.split('<script id="aggr-preferences">')[1].split("</script>")[0].replace("{{ site.preferences | json }}", "{}");
  const global: { AGGRPreferences?: Preferences } = {};
  runInNewContext(script, { window: global, document: { documentElement: { dataset: {} } }, localStorage: { getItem: () => null } });
  return global.AGGRPreferences!.schema as BootstrapSchema;
}
