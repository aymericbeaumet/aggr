import { afterEach, describe, expect, it, vi } from 'vitest';
import { createMedia } from './media';

class TestElement extends EventTarget {
  dataset: Record<string, string> = {};
  attributes = new Map<string, string>();
  classes = new Set<string>();
  children: TestElement[] = [];
  parent?: TestElement;
  hidden = false;
  src = ''; title = ''; loading = ''; allow = ''; allowFullscreen = false; referrerPolicy = '';
  focus = vi.fn();
  classList = {
    contains: (name: string) => this.classes.has(name),
    add: (...names: string[]) => { for (const name of names) this.classes.add(name); },
    remove: (...names: string[]) => { for (const name of names) this.classes.delete(name); },
  };
  setAttribute(name: string, value: string) { this.attributes.set(name, value); }
  getAttribute(name: string) { return this.attributes.get(name) ?? null; }
  hasAttribute(name: string) { return this.attributes.has(name); }
  removeAttribute(name: string) { this.attributes.delete(name); }
  closest() { return this.parent; }
  appendChild(child: TestElement) { this.children.push(child); child.parent = this; }
  click() { this.dispatchEvent(Object.assign(new Event('click'), { button: 0 })); }
}

function facade(label: string) {
  const frames: TestElement[] = [];
  vi.stubGlobal('document', { createElement: () => { const frame = new TestElement(); frames.push(frame); return frame; } });
  const player = new TestElement();
  player.dataset.videoProvider = 'vimeo';
  player.classList.add('video-player', 'video-player-inline');
  const preview = new TestElement();
  preview.parent = player;
  preview.dataset.videoEmbed = 'https://player.vimeo.com/video/123';
  preview.setAttribute('aria-label', label);
  const media = createMedia({ base: () => 'https://reader.test/', kind: () => 'item' });
  const root = { querySelectorAll: () => [preview] } as unknown as ParentNode;
  return { media, root, player, preview, frames };
}

afterEach(() => vi.unstubAllGlobals());

describe('video facades', () => {
  it('titles the player with the preview label from before external-link decoration', () => {
    const { media, root, player, preview, frames } = facade('Play Launch on Vimeo, opens in a new tab');
    preview.dataset.aggrExternalLabel = 'Play Launch on Vimeo';
    media.enhanceVideo(root);
    expect(preview.getAttribute('role')).toBe('button');
    preview.click();
    expect(frames).toHaveLength(1);
    expect(frames[0].title).toBe('Play Launch on Vimeo');
    expect(frames[0].src).toBe('https://player.vimeo.com/video/123?autoplay=1');
    expect(player.children).toEqual([frames[0]]);
  });

  it('keeps the label it bound with when the link is decorated only afterwards', () => {
    const { media, root, preview, frames } = facade('Play Launch on Vimeo');
    media.enhanceVideo(root);
    preview.setAttribute('aria-label', 'Play Launch on Vimeo, opens in a new tab');
    preview.click();
    expect(frames[0].title).toBe('Play Launch on Vimeo');
  });
});
