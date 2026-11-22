/** JSON in an HTML script must never introduce an HTML end tag. */
export function jsonScript(id: string | null, value: unknown, type = 'application/json'): string {
  const json = JSON.stringify(value).replace(/</g, '\\u003c').replace(/>/g, '\\u003e').replace(/&/g, '\\u0026');
  return `<script${id ? ` id="${id}"` : ''} type="${type}">${json}</script>`;
}
