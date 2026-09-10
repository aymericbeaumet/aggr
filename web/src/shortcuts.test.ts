import { describe, expect, it, vi } from 'vitest';
import { bindShortcutDialog } from './shortcuts';

class TestDialog extends EventTarget {
  open = false;
  trigger = { isConnected: true, focus: vi.fn() };
  heading = { focus: vi.fn() };
  ownerDocument = { activeElement: this.trigger };
  showModal = vi.fn(() => { this.open = true; });
  close = vi.fn(() => { this.open = false; this.dispatchEvent(new Event('close')); });
  querySelector = vi.fn(() => this.heading);
}

function setup() {
  const dialog = new TestDialog();
  return { dialog, help: bindShortcutDialog(dialog as unknown as HTMLDialogElement) };
}

describe('shortcut help lifecycle', () => {
  it('opens once, focuses the heading, and restores its trigger on close', () => {
    const { dialog, help } = setup();
    help.open();
    help.open();
    expect(dialog.showModal).toHaveBeenCalledTimes(1);
    expect(dialog.heading.focus).toHaveBeenCalledWith({ preventScroll: true });
    help.close();
    expect(dialog.trigger.focus).toHaveBeenCalledTimes(1);
    help.dispose();
  });

  it('closes on backdrop click and handles native Escape closure', () => {
    const { dialog, help } = setup();
    help.toggle();
    dialog.dispatchEvent(new Event('click'));
    expect(dialog.open).toBe(false);
    help.open();
    dialog.close();
    expect(dialog.trigger.focus).toHaveBeenCalledTimes(2);
    help.dispose();
  });

  it('disposes without stealing focus and releases backdrop and close handlers', () => {
    const { dialog, help } = setup();
    help.open();
    help.dispose();
    expect(dialog.trigger.focus).not.toHaveBeenCalled();
    dialog.close.mockClear();
    dialog.dispatchEvent(new Event('click'));
    dialog.dispatchEvent(new Event('close'));
    help.open();
    expect(dialog.close).not.toHaveBeenCalled();
    expect(dialog.showModal).toHaveBeenCalledTimes(1);
  });

  it('does not restore a detached trigger or let a stale close event affect a reopened dialog', () => {
    const { dialog, help } = setup();
    help.open();
    dialog.trigger.isConnected = false;
    help.close();
    expect(dialog.trigger.focus).not.toHaveBeenCalled();
    dialog.trigger.isConnected = true;
    help.open();
    dialog.dispatchEvent(new Event('close'));
    expect(dialog.trigger.focus).not.toHaveBeenCalled();
    help.close();
    expect(dialog.trigger.focus).toHaveBeenCalledTimes(1);
    help.dispose();
  });
});
