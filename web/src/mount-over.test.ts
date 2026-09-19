import { describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({ unmount: vi.fn(async (_component: unknown) => {}) }));
vi.mock('svelte', () => ({ unmount: mocks.unmount }));
import { mountOver } from './mount-over';

function target(children: string[]) {
  const nodes = children.map(text => ({ text })) as unknown as ChildNode[];
  const element = { childNodes: [...nodes], replaceChildren: vi.fn((...next: ChildNode[]) => { element.childNodes = next; }) };
  return { element, nodes };
}

describe('mounting over server-rendered markup', () => {
  it('snapshots the fallback once, clears the target and exposes the components in order', async () => {
    const { element, nodes } = target(['form', 'hint']);
    const first = { name: 'first' }, second = { name: 'second' };
    const overlay = mountOver(element as unknown as ParentNode, [() => first, () => second]);
    expect(overlay.mounted).toEqual([first, second]);
    expect(overlay.fallback).toEqual(nodes);
    expect(element.replaceChildren).toHaveBeenCalledTimes(1);
    expect(element.childNodes).toEqual([]);
    await overlay.restore();
    expect(mocks.unmount.mock.calls.map(call => call[0])).toEqual([first, second]);
    expect(element.childNodes).toEqual(nodes);
  });

  it('restores the fallback and unmounts what was mounted when a mount throws', () => {
    mocks.unmount.mockClear();
    const { element, nodes } = target(['form']);
    const first = { name: 'first' };
    expect(() => mountOver(element as unknown as ParentNode, [() => first, () => { throw new Error('boom'); }])).toThrow('boom');
    expect(mocks.unmount).toHaveBeenCalledTimes(1);
    expect(mocks.unmount).toHaveBeenCalledWith(first);
    expect(element.childNodes).toEqual(nodes);
  });

  it('tolerates a rejected unmount while rolling back', async () => {
    mocks.unmount.mockClear();
    mocks.unmount.mockImplementationOnce(() => Promise.reject(new Error('teardown failed')));
    const { element, nodes } = target(['form']);
    const rejections = vi.fn();
    process.on('unhandledRejection', rejections);
    try {
      expect(() => mountOver(element as unknown as ParentNode, [() => ({}), () => { throw new Error('boom'); }])).toThrow('boom');
      await new Promise(resolve => setTimeout(resolve, 0));
      expect(rejections).not.toHaveBeenCalled();
      expect(element.childNodes).toEqual(nodes);
    } finally { process.off('unhandledRejection', rejections); }
  });
});
