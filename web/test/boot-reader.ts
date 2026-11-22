import { testTools } from './browser';
import { badges } from '../src/updates/badges';
import { page } from '../src/state/page.svelte';
import { versions } from '../src/state/versions.svelte';
export const startReader = () => badges.configure(page.root, sessionStorage, document);
export const pageArrived = testTools.fn(() => {
  versions.latest = String(Number(versions.latest || 0) + 1);
  const model = page.model;
  badges.arrive([...(model?.site.entries ?? []), ...(model?.page.view === 'list' ? model.page.data.rows.map(row => row.url) : [])]);
});
export function refreshToUpdate() {}
export function retryConnection() {}
