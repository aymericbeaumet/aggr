import { describe, expect, it } from 'vitest';
import { ScrollMemory, newKey } from './scroll';

describe('ScrollMemory', () => {
  it('keeps one place per entry and the last fifty lists', () => {
    const memory = new ScrollMemory(3);
    memory.remember('e1', 120);
    expect(memory.recall('e1')).toBe(120);
    expect(memory.recall('e2')).toBeUndefined();
    memory.leaveList('a', 1);
    memory.leaveList('b', 2);
    memory.leaveList('c', 3);
    memory.leaveList('a', 4);
    memory.leaveList('d', 5);
    expect(memory.place('b')).toBeUndefined();
    expect(memory.place('a')).toBe(4);
    expect(memory.place('d')).toBe(5);
  });
});

describe('newKey', () => {
  it('differs between calls', () => {
    expect(newKey()).not.toBe(newKey());
  });
});
