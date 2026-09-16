import { mount } from 'svelte';
import ConnectionStatus from './components/ConnectionStatus.svelte';
import { mountOver } from './mount-over';
import type { Updates } from './updates';

export function mountConnectionStatus(root: ParentNode, model: Updates, actions: { retry(): void; refresh(): void }) {
  const target = root.querySelector<HTMLElement>('[data-connection-root]');
  if (target) {
    const overlay = mountOver(target, [() => mount(ConnectionStatus, { target, props: { model, ...actions } })]);
    return () => overlay.restore();
  }
  return () => {};
}
