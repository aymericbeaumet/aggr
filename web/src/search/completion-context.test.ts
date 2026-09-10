import { describe, expect, it, vi } from 'vitest';
import { completionContext, LatestFacets } from './completion-context';
import type { Facet } from './types';

describe('completion context', () => {
  it('removes only the edited token, preserving every other filter and full-text clause', () => {
    const raw = 'category:programming "memory safety" source:"un -tag:ads type:podcast';
    // An unfinished quote consumes the remaining text, so use a complete token when
    // editing before later clauses; an unfinished final token is also supported.
    const query = raw.replace('source:"un ', 'source:un ');
    const context = completionContext(query, query.indexOf('source:un') + 9)!;
    expect(context.field).toBe('source');
    expect(context.query.clauses.map(clause => [clause.kind, clause.value])).toEqual([
      ['facet','programming'],['text','memory safety'],['facet','ads'],['facet','podcast']
    ]);
    expect(completionContext('category:programming source:"Un', 31)?.query.raw.trim()).toBe('category:programming');
    expect(completionContext('source:un', 9)?.query.clauses).toEqual([]);
    expect(completionContext('rust date:>=2026-', 17)?.field).toBe('published-day');
    expect(() => completionContext('category: source:', 17)).toThrow(/Add a value/);
  });
});

describe('asynchronous completion freshness', () => {
  it('ignores late results after a different context wins and cancels obsolete work', async () => {
    const latest = new LatestFacets();
    let first!: (value: Facet[]) => void;
    let oldSignal!: AbortSignal;
    const apply = vi.fn(), failed = vi.fn();
    latest.request('programming', signal => { oldSignal = signal; return new Promise(resolve => { first = resolve; }); }, apply, failed);
    latest.request('news', async () => [{value:'news',label:'News',count:2}], apply, failed);
    await Promise.resolve();
    first([{value:'old',label:'Old',count:99}]);
    await Promise.resolve();
    expect(oldSignal.aborted).toBe(true);
    expect(apply).toHaveBeenCalledTimes(1);
    expect(apply).toHaveBeenCalledWith([{value:'news',label:'News',count:2}]);
    expect(failed).not.toHaveBeenCalled();
  });
  it('deduplicates repeated context loads and ignores failures after clearing or changing index', async () => {
    const latest = new LatestFacets();
    let reject!: (error: Error) => void;
    const load = vi.fn(() => new Promise<Facet[]>((_resolve, fail) => { reject = fail; }));
    const apply = vi.fn(), failed = vi.fn();
    latest.request('v1:source', load, apply, failed);
    latest.request('v1:source', load, apply, failed);
    expect(load).toHaveBeenCalledTimes(1);
    latest.clear();
    reject(new Error('old index unavailable'));
    await Promise.resolve();
    await Promise.resolve();
    expect(failed).not.toHaveBeenCalled();
    latest.request('v2:source', async () => [], apply, failed);
    await Promise.resolve();
    expect(apply).toHaveBeenCalledWith([]);
  });
});
