import { beforeEach, describe, expect, it } from 'vitest';
import { announcer } from '../state/announcer.svelte';
import { fresh } from '../state/fresh.svelte';
import { Badges, lastSeenKey, newEntriesKey } from './badges';

const root = 'https://reader.test/reader/';
const a = `${root}items/a/`;
const b = `${root}items/b/`;
const c = `${root}items/c/`;
const frame = () => new Promise((resolve) => requestAnimationFrame(resolve));

describe('Badges', () => {
  beforeEach(() => {
    sessionStorage.clear();
    fresh.pending = [];
    fresh.marked = [];
    document.title = 'reader';
  });

  it('remembers the head and lights what arrived above it', async () => {
    sessionStorage.setItem(lastSeenKey('/reader/'), 'items/b/');
    const badges = new Badges();
    badges.configure(root, sessionStorage, document);
    badges.arrive(['items/a/', 'items/b/', 'items/c/']);
    expect(fresh.pending).toEqual([a]);
    expect(sessionStorage.getItem(lastSeenKey('/reader/'))).toBe(a);
    expect(sessionStorage.getItem(newEntriesKey('/reader/'))).toBe(JSON.stringify([a]));
    // The rows on screen do not hold it yet: nothing is marked.
    badges.reveal([b, c]);
    expect(fresh.marked).toEqual([]);
    badges.reveal([a, b]);
    expect(fresh.marked).toEqual([a]);
    expect(fresh.pending).toEqual([]);
    expect(sessionStorage.getItem(newEntriesKey('/reader/'))).toBe('[]');
    await frame();
    expect(announcer.message).toBe('1 new item');
    // A visible tab carries no dot.
    expect(document.title).toBe('reader');
  });

  it('starts from what the session still had pending, and a page arriving clears the marks', () => {
    sessionStorage.setItem(newEntriesKey('/reader/'), JSON.stringify([c]));
    const badges = new Badges();
    badges.configure(root, sessionStorage, document);
    expect(fresh.pending).toEqual([c]);
    fresh.marked = [a];
    badges.arrive(['items/a/']);
    expect(fresh.marked).toEqual([]);
    // Without a remembered head nothing more is new.
    expect(fresh.pending).toEqual([c]);
  });

  it('does nothing before it is bound to a site', () => {
    const badges = new Badges();
    badges.observe(['items/a/']);
    badges.reveal([a]);
    expect(fresh.pending).toEqual([]);
    expect(fresh.marked).toEqual([]);
  });
});
