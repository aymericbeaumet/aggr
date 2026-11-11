import { render as renderComponent } from 'svelte/server';
import type { RenderRequest } from '@aggr-model/RenderRequest';
import App from './app/App.svelte';
import { urlFor } from './model/urls';

const attribute = (value: string) => value.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');

export function render(request: RenderRequest): string {
  const result = renderComponent(App, { props: { page: request.page, bodyHtml: request.body_html } });
  const language = attribute(request.page.site.language);
  const src = request.page.site.assets.app;
  const script = src ? `<script type="module" src="${attribute(urlFor(request.page.base, src))}"></script>` : '';
  return `<!doctype html><html lang="${language}"><head><meta charset="utf-8">${result.head}</head><body data-kind="${attribute(request.page.kind)}"><div id="app">${result.body}</div>${script}</body></html>`;
}
