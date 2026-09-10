export type PreferenceValue = string | number | boolean;
export type PreferenceValues = Record<string, PreferenceValue>;
export interface Preferences {
  values: PreferenceValues & { "date-format": string; "single-key-shortcuts": boolean; "scroll-amount": number; "offline-items": number; theme: string };
  schema: Record<string, { attribute?: string; initial: PreferenceValue }>;
  valid(key: string, value: unknown): boolean;
  validate(value: unknown): PreferenceValues;
  read(): PreferenceValues;
}
export interface AppContext {
  base?: string; kind?: string; pwa?: boolean; appVersion?: string; contentVersion?: string;
  entries?: string[]; discussions?: Array<{ name: string; shortcut?: string; url: string }>;
}
export interface BuildManifest { app_version?: unknown; content_version?: unknown; entries?: unknown }
export interface ListPosition { url?: string; y?: number }
export interface ArticleHeader {
  header: HTMLElement; title: HTMLElement | null; tags: HTMLElement | null;
  labels: HTMLElement | null; progress: HTMLElement | null; fade: HTMLElement | null; topBar: HTMLElement | null;
  scrollRange: number; folded?: boolean;
}
export interface NavigationPage { url: string; html: string }
export interface NavigationOptions extends RequestInit { priority?: RequestPriority; prefetchSignal?: AbortSignal }
export interface NavigationRequest {
  speculative: boolean; controller?: AbortController; promise: Promise<NavigationPage>;
}
// The vendored Swup hook payloads are adapted at this external boundary.
export interface SwupAdapter {
  navigating?: boolean;
  location: URL;
  navigate(target: string): unknown;
  fetchPage(target: string, options?: NavigationOptions): Promise<NavigationPage>;
  hooks: { on(name: string, callback: (...args: any[]) => unknown): void; before(name: string, callback: (...args: any[]) => unknown): void };
  cache: { has(url: string): boolean; set(url: string, page: NavigationPage): void; delete(url: string): void; clear(): void; size: number; all: Map<string, NavigationPage> };
}
export interface ReaderWindow extends Window {
  AGGR: AppContext;
  AGGRPreferences: Preferences;
  Swup?: new (options: Record<string, unknown>) => SwupAdapter;
  swup?: SwupAdapter;
}
export interface ReaderNavigator extends Navigator {
  standalone?: boolean;
  connection?: { saveData?: boolean; effectiveType?: string };
}
