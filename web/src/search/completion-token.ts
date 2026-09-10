import { tokenize } from './query';

function analyze(query: string, cursor: number) {
  const token = tokenize(query, true).find(token => token.start <= cursor && cursor <= token.end);
  const start = token?.start ?? cursor, end = token?.end ?? cursor;
  const decoded = tokenize(query.slice(start, cursor), true)[0]?.value || '';
  const excluded = decoded.startsWith('-') ? '-' : '';
  return { token, start, end, excluded, value: excluded ? decoded.slice(1) : decoded };
}

let latest: { query: string; cursor: number; value: ReturnType<typeof analyze> } | undefined;

export function completionToken(query: string, cursor: number) {
  if (latest?.query !== query || latest.cursor !== cursor) latest = { query, cursor, value: analyze(query, cursor) };
  return latest.value;
}
