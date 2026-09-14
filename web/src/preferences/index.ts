import { flushSync, mount, unmount } from "svelte";
import Panel from "./Panel.svelte";
import { createPreferenceActions } from "./actions";
import type { PreferencesService } from "./service";
export { createPreferencesService } from "./service";

export function mountPreferences(root: ParentNode, service: PreferencesService, options: { onShortcuts: () => void }): { dispose: () => void | Promise<void> } {
  const page = root.querySelector<HTMLElement>(".preferences");
  if (!page) return { dispose() {} };
  const target = page.querySelector<HTMLElement>("[data-preferences-root]");
  if (!target) return { dispose() {} };
  const actions = createPreferenceActions(page, service);
  const fallback = Array.from(target.childNodes);
  target.replaceChildren();
  const component = mount(Panel, { target, props: { service, actions, onShortcuts: options.onShortcuts } });
  flushSync();
  return { async dispose() {
    actions.dispose();
    await unmount(component);
    target.replaceChildren(...fallback);
  } };
}
