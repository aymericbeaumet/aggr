import { render } from './server';
(globalThis as unknown as { aggrRender: (json: string) => string }).aggrRender = (json) => render(JSON.parse(json));
