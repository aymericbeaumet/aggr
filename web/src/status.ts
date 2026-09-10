import { mount, unmount } from 'svelte';
import ConnectionStatus from './components/ConnectionStatus.svelte';
import type { Updates } from './updates';

export function mountConnectionStatus(root: ParentNode, model: Updates, actions: { retry(): void; refresh(): void }) {
  const target = root.querySelector<HTMLElement>('[data-connection-root]');
  if (target) {
    const fallback = [...target.childNodes];
    target.replaceChildren();
    const component = mount(ConnectionStatus, { target, props: { model, ...actions } });
    return async () => { await unmount(component); target.replaceChildren(...fallback); };
  }
  return () => {};
}
