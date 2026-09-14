import { afterEach, describe, expect, it, vi } from 'vitest';
import { enhanceInteractive, interactiveUrl } from './interactive';

class TestElement extends EventTarget {
  dataset: Record<string, string> = {};
  attributes = new Map<string, string>();
  children: TestElement[] = [];
  parent?: TestElement;
  hidden = false;
  src = '';
  title = '';
  referrerPolicy = '';
  focus = vi.fn();
  setAttribute(name: string, value: string) { this.attributes.set(name, value); }
  getAttribute(name: string) { return this.attributes.get(name) ?? null; }
  removeAttribute(name: string) { this.attributes.delete(name); }
  closest() { return this.parent; }
  querySelector() { return this.children[0]; }
  appendChild(child: TestElement) { this.children.push(child); child.parent = this; }
  remove() { if (this.parent) this.parent.children = this.parent.children.filter(child => child !== this); }
}

function fixture(url = 'https://publisher.test/diagram/?mode=3d#figure') {
  const container = new TestElement();
  const link = new TestElement();
  link.parent = container;
  link.dataset.interactiveEmbed = url;
  link.setAttribute('aria-label', 'Interactive diagram');
  const frames: TestElement[] = [];
  vi.stubGlobal('document', { createElement: vi.fn(() => {
    const frame = new TestElement();
    frames.push(frame);
    return frame;
  }) });
  const root = { querySelectorAll: () => [link] } as unknown as ParentNode;
  return { container, link, frames, root, controller: new AbortController() };
}

afterEach(() => vi.unstubAllGlobals());

describe('interactive originals', () => {
  it('preserves complete public URLs', () => {
    const url = 'https://publisher.test/diagram/?mode=3d#figure';
    expect(interactiveUrl(url)?.href).toBe(url);
  });

  it('rejects executable, relative, and credentialed sources', () => {
    for (const value of [undefined, '', '/app', '//publisher.test/app', 'javascript:alert(1)', 'data:text/html,test', 'file:///app', 'https://user:secret@publisher.test/app']) {
      expect(interactiveUrl(value)).toBeUndefined();
    }
  });

  it('loads the original immediately in its sandbox without taking focus or duplicating the frame', () => {
    const { container, link, frames, root, controller } = fixture();
    enhanceInteractive(root, controller.signal);
    enhanceInteractive(root, controller.signal);
    expect(frames).toHaveLength(1);
    const [frame] = frames;
    expect(container.children).toEqual([frame]);
    expect(frame.src).toBe(link.dataset.interactiveEmbed);
    expect(frame.title).toBe('Interactive diagram');
    expect(frame.getAttribute('sandbox')).toBe('allow-scripts');
    expect(frame.referrerPolicy).toBe('no-referrer');
    expect(frame.focus).not.toHaveBeenCalled();
    expect(link.getAttribute('role')).toBeNull();
    expect(container.getAttribute('aria-busy')).toBe('true');
    frame.dispatchEvent(new Event('load'));
    expect(container.getAttribute('aria-busy')).toBeNull();
    expect(link.hidden).toBe(true);
    controller.abort();
  });

  it('releases the live original on disposal and ignores its late load event', () => {
    const { container, link, frames, root, controller } = fixture();
    enhanceInteractive(root, controller.signal);
    expect(frames).toHaveLength(1);
    controller.abort();
    frames[0].dispatchEvent(new Event('load'));
    expect(container.children).toEqual([]);
    expect(container.getAttribute('aria-busy')).toBeNull();
    expect(link.hidden).toBe(false);
    expect(link.dataset.interactiveBound).toBeUndefined();
    const next = new AbortController();
    enhanceInteractive(root, next.signal);
    expect(frames).toHaveLength(2);
    next.abort();
  });

  it('keeps invalid originals and already disposed pages as ordinary links', () => {
    const { root, controller, frames, link } = fixture('javascript:alert(1)');
    enhanceInteractive(root, controller.signal);
    expect(frames).toHaveLength(0);
    link.dataset.interactiveEmbed = 'https://publisher.test/app';
    controller.abort();
    enhanceInteractive(root, controller.signal);
    expect(frames).toHaveLength(0);
    expect(link.hidden).toBe(false);
  });
});
