/** Parse the JSON model a page embeds; throws on malformed input like `JSON.parse`. */
export function parse(text: string): unknown {
  return JSON.parse(text) as unknown;
}
