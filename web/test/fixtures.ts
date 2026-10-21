import type { ClientPage } from '@aggr-model/ClientPage';
import type { RenderRequest } from '@aggr-model/RenderRequest';

declare global { var aggrRender: (json: string) => string; var __aggrFixtures: Record<string, ClientPage>; }
export const render = (request: RenderRequest): string => globalThis.aggrRender(JSON.stringify(request));
export function fixture(name: string): ClientPage {
  return structuredClone(globalThis.__aggrFixtures[name]);
}
export function load(name: string, bodyHtml = ''): ClientPage {
  const page = fixture(name);
  const doc = new DOMParser().parseFromString(render({ page, body_html: bodyHtml }), 'text/html');
  document.head.innerHTML = doc.head.innerHTML;
  document.body.innerHTML = doc.body.innerHTML;
  return page;
}
