import { flushSync, mount } from "svelte";
import Panel from "./Panel.svelte";
import { mountOver } from "../mount-over";
import { createPreferenceActions } from "./actions";
import type { PreferencesService } from "./service";
export { createPreferencesService } from "./service";

export function mountPreferences(root: ParentNode, service: PreferencesService, options: { onShortcuts: () => void }): { dispose: () => void | Promise<void> } {
  const page = root.querySelector<HTMLElement>(".preferences");
  if (!page) return { dispose() {} };
  const target = page.querySelector<HTMLElement>("[data-preferences-root]");
  if (!target) return { dispose() {} };
  const actions = createPreferenceActions(page, service);
  let overlay: ReturnType<typeof mountOver>;
  try {
    overlay = mountOver(target, [() => mount(Panel, { target, props: { service, actions, onShortcuts: options.onShortcuts } })]);
  } catch (error) {
    actions.dispose();
    throw error;
  }
  flushSync();
  return { async dispose() {
    actions.dispose();
    await overlay.restore();
  } };
}
