/**
 * Full-text search: one lazily loaded chunk. `state/search.svelte.ts` imports it on the first
 * intent and hands the components its driver; nothing here runs before someone searches.
 */
export { driver, DEBOUNCE } from './controller';
export type { Completion } from './completion';
