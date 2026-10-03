import { describe, expect, it } from 'vitest';
import { adopt } from './adopt';

describe('adopt', () => {
  it('moves the same nodes into the wrapper and back out on teardown', () => {
    const source = document.createElement('div');
    source.innerHTML = '<p>One</p><figure>Two</figure>';
    const [p, figure] = [...source.childNodes];
    const wrapper = document.createElement('div');
    wrapper.append(document.createComment('anchor'));
    const attachment = adopt(source);
    expect(attachment).not.toBeNull();
    const teardown = attachment?.(wrapper);
    expect(source.childNodes.length).toBe(0);
    expect(wrapper.childNodes[0].nodeType).toBe(Node.COMMENT_NODE);
    expect(wrapper.childNodes[1]).toBe(p);
    expect(wrapper.childNodes[2]).toBe(figure);
    if (typeof teardown === 'function') teardown();
    expect(wrapper.childNodes.length).toBe(1);
  });

  it('places the captured nodes again in a rebuilt wrapper', () => {
    const source = document.createElement('div');
    source.innerHTML = '<p>One</p>';
    const p = source.firstChild;
    const first = document.createElement('div');
    adopt(source)?.(first);
    const second = document.createElement('div');
    adopt(source)?.(second);
    expect(second.firstChild).toBe(p);
    expect(first.childNodes.length).toBe(0);
  });

  it('is nothing without content', () => {
    expect(adopt(null)).toBeNull();
    expect(adopt(undefined)).toBeNull();
  });
});
