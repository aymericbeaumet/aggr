import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { runInNewContext } from 'node:vm';
import type { ClientPage } from '@aggr-model/ClientPage';
import type { RenderRequest } from '@aggr-model/RenderRequest';

const runtime: { aggrRender?: (json: string) => string } = {};
runInNewContext(readFileSync(resolve(process.cwd(), '.generated/dist/renderer.js'), 'utf8'), runtime);
export const render = (request: RenderRequest): string => runtime.aggrRender!(JSON.stringify(request));
export function fixture(name: string): ClientPage {
  return JSON.parse(readFileSync(resolve(process.cwd(), `test/fixtures/${name}.json`), 'utf8'));
}
export function load(name: string, bodyHtml = ''): ClientPage {
  const page = fixture(name);
  const doc = new DOMParser().parseFromString(render({ page, body_html: bodyHtml }), 'text/html');
  document.head.innerHTML = doc.head.innerHTML;
  document.body.innerHTML = doc.body.innerHTML;
  return page;
}
