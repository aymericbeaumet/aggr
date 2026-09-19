import { flushSync, mount, unmount } from 'svelte';
import ShortcutHelp from './components/ShortcutHelp.svelte';
import { mountOver } from './mount-over';
import type { AppContext } from './contracts';

export interface ShortcutHelpHandle {
  open(): void;
  close(): void;
  toggle(): void;
  dispose(): Promise<void>;
}

export function bindShortcutDialog(dialog: HTMLDialogElement) {
  const listeners = new AbortController();
  let previousFocus: HTMLElement | null = null;
  let disposed = false;

  function restoreFocus() {
    if (previousFocus?.isConnected) previousFocus.focus({ preventScroll: true });
    previousFocus = null;
  }
  function open() {
    if (disposed || dialog.open) return;
    previousFocus = dialog.ownerDocument.activeElement as HTMLElement | null;
    dialog.showModal();
    dialog.querySelector<HTMLElement>('#shortcut-help-title')?.focus({ preventScroll: true });
  }
  function close() {
    if (disposed || !dialog.open) return;
    dialog.close();
    restoreFocus();
  }
  dialog.addEventListener('close', () => { if (!dialog.open) restoreFocus(); }, { signal: listeners.signal });
  dialog.addEventListener('click', event => { if (event.target === dialog) close(); }, { signal: listeners.signal });
  return {
    open, close,
    toggle() { if (dialog.open) close(); else open(); },
    dispose() {
      disposed = true;
      listeners.abort();
      previousFocus = null;
      if (dialog.open) {
        dialog.close();
      }
    },
  };
}

/** A handle that does nothing: what the page keeps when the shortcut dialog could not be mounted. */
export function inertShortcutHelp(): ShortcutHelpHandle {
  return { open() {}, close() {}, toggle() {}, async dispose() {} };
}

export function mountShortcutHelp(root: ParentNode = document, discussions: AppContext['discussions'] = []): ShortcutHelpHandle {
  const target = root.querySelector<HTMLElement>('[data-shortcut-help-root]');
  let component: ReturnType<typeof mount> | undefined;
  if (target) {
    // The static dialog markup comes back if the mount fails; a successful mount owns it until disposal.
    [component] = mountOver(target, [() => mount(ShortcutHelp, { target, props: { discussions } })]).mounted;
    flushSync();
  }
  const dialog = target?.querySelector<HTMLDialogElement>('#shortcut-help');
  const controller = dialog ? bindShortcutDialog(dialog) : undefined;
  let disposal: Promise<void> | undefined;
  return {
    open() { controller?.open(); },
    close() { controller?.close(); },
    toggle() { controller?.toggle(); },
    dispose() {
      return disposal ??= (async () => {
        controller?.dispose();
        if (component) await unmount(component);
      })();
    },
  };
}
