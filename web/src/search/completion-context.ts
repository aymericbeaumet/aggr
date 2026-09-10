import { parseQuery } from './query';
import { completionToken } from './completion-token';
import type { Facet, FilterKind, Query } from './types';

export function completionContext(raw: string, cursor: number): {field: FilterKind; query: Query} | undefined {
  const { token, value } = completionToken(raw, cursor);
  if (!token) return;
  const operator = value.match(/^(source|category|tag|type|date|before|after|since|until):/i)?.[1].toLowerCase();
  if (!operator) return;
  const field: FilterKind = ['date','before','after','since','until'].includes(operator) ? 'published-day' : operator as FilterKind;
  return { field, query: parseQuery(raw.slice(0, token.start) + raw.slice(token.end)) };
}

export class LatestFacets {
  private current?: {key: string; controller: AbortController};

  clear() { this.current?.controller.abort(); this.current = undefined; }

  request(key: string, load: (signal: AbortSignal) => Promise<Facet[]>, apply: (facets: Facet[]) => void, fail: (error: unknown) => void) {
    if (this.current?.key === key) return;
    this.clear();
    const current = {key, controller: new AbortController()};
    this.current = current;
    void load(current.controller.signal).then(facets => {
      if (this.current === current) apply(facets);
    }).catch(error => { if (this.current === current) fail(error); });
  }
}
