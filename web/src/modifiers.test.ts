import { describe, expect, it } from 'vitest';
import { installShiftHover } from './modifiers';

function fixture() {
  const classes = new Set<string>();
  const document = Object.assign(new EventTarget(), {
    hidden: false,
    documentElement: { classList: { toggle(name: string, enabled: boolean) { enabled ? classes.add(name) : classes.delete(name); } } }
  });
  const window = new EventTarget();
  const dispose = installShiftHover(document as unknown as Document, window as unknown as Window);
  const send = (target: EventTarget, type: string, properties = {}) => target.dispatchEvent(Object.assign(new Event(type), properties));
  return { document, window, dispose, send, held: () => classes.has('is-shift-held') };
}

describe('Shift link hover', () => {
  it('updates on Shift without pointer movement and keeps another held Shift active', () => {
    const f = fixture();
    f.send(f.document, 'keydown', { key: 'Shift', shiftKey: true });
    expect(f.held()).toBe(true);
    f.send(f.document, 'keyup', { key: 'Shift', shiftKey: true });
    expect(f.held()).toBe(true);
    f.send(f.document, 'keyup', { key: 'Shift', shiftKey: false });
    expect(f.held()).toBe(false);
    f.dispose();
  });

  it('clears lost key releases on blur, hidden pages, and pagehide', () => {
    const f = fixture();
    for (const [target, type] of [[f.window, 'blur'], [f.document, 'visibilitychange'], [f.window, 'pagehide']] as const) {
      f.send(f.document, 'keydown', { key: 'Shift', shiftKey: true });
      f.document.hidden = true;
      f.send(target, type, { persisted: true });
      expect(f.held()).toBe(false);
    }
    f.dispose();
  });

  it('recovers the modifier from pointer entry and removes all listeners when disposed', () => {
    const f = fixture();
    f.send(f.document, 'pointerover', { shiftKey: true });
    expect(f.held()).toBe(true);
    f.dispose();
    expect(f.held()).toBe(false);
    f.send(f.document, 'keydown', { key: 'Shift', shiftKey: true });
    f.send(f.document, 'pointerover', { shiftKey: true });
    expect(f.held()).toBe(false);
  });
});
