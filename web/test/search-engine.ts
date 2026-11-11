import { hex, row, searchRow } from '../src/search/fixture';
import type { ClientRow } from '@aggr-model/ClientRow';
import type { Index, SearchPage, Session } from '../src/search/engine';
import type { Catalog } from '../src/search/types';

export const fake = { results: [] as { url: string; title: string }[], runs: [] as string[] };
const manifest: Catalog = { version: 'v1', base: 'pagefind/v1/', docs: 3, facets: { source: [{ value: 'blog.example', label: 'Blog', count: 3 }] } };
const index: Index = {
  manifest, retire() {}, async prepare() {},
  async run(query): Promise<SearchPage> {
    fake.runs.push(query.raw);
    const results = fake.results.map(({ url, title }) => ({ url, meta: { title, aggr_display: hex(searchRow({ ...row, url, path: url.replace(/\/$/, ''), title } satisfies ClientRow)) } }));
    return { results, total: results.length, page: 1, pages: 1, size: 50 };
  },
  async counts() { return []; },
};
const session: Session = { load: async () => index, stale() {}, network: () => false, dispose() {} };
export const createSession = () => session;
export const isOnline = () => true;
