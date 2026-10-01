import { describe, expect, it } from 'vitest';
import { digest, files, harness, indexBase, manifest, scope, version } from './harness';
import { completeSearch, configureOfflineSearch, validSearchManifest, verifiedSearchResponse } from './search';
import type { SearchManifest } from './types';

const variant = (over: Partial<SearchManifest>): unknown => ({ ...manifest, ...over });

describe('validSearchManifest', () => {
  it('accepts the published manifest', () => {
    expect(validSearchManifest(scope, manifest)).toBe(true);
  });

  it('rejects malformed versions, bases and file lists', () => {
    expect(validSearchManifest(scope, null)).toBe(false);
    expect(validSearchManifest(scope, [])).toBe(false);
    expect(validSearchManifest(scope, variant({ version: 'latest' }))).toBe(false);
    expect(validSearchManifest(scope, variant({ version: 'A'.repeat(64) }))).toBe(false);
    expect(validSearchManifest(scope, variant({ base: 'pagefind/' }))).toBe(false);
    expect(validSearchManifest(scope, variant({ base: `pagefind/${'c'.repeat(64)}/` }))).toBe(false);
    expect(validSearchManifest(scope, variant({ files: [] }))).toBe(false);
    expect(validSearchManifest(scope, { ...manifest, files: null })).toBe(false);
  });

  it('rejects files outside the base, duplicates, and unsafe or inconsistent sizes and digests', () => {
    const [runtime, ...rest] = manifest.files;
    const withFile = (file: unknown) => variant({ files: [runtime, ...rest, file] as SearchManifest['files'] });
    expect(validSearchManifest(scope, withFile({ ...runtime }))).toBe(false);
    expect(validSearchManifest(scope, withFile({ ...runtime, url: `${indexBase}../escape.js` }))).toBe(false);
    expect(validSearchManifest(scope, withFile({ ...runtime, url: `${indexBase}query.js?x=1` }))).toBe(false);
    expect(validSearchManifest(scope, withFile({ ...runtime, url: `${indexBase}anchor.js#top` }))).toBe(false);
    expect(validSearchManifest(scope, withFile({ ...runtime, url: 'assets/app/other.js' }))).toBe(false);
    expect(validSearchManifest(scope, withFile({ ...runtime, url: `${indexBase}neg.js`, size: -1 }))).toBe(false);
    expect(validSearchManifest(scope, withFile({ ...runtime, url: `${indexBase}frac.js`, size: 1.5 }))).toBe(false);
    expect(validSearchManifest(scope, withFile({ ...runtime, url: `${indexBase}big.js`, size: Number.MAX_SAFE_INTEGER }))).toBe(false);
    expect(validSearchManifest(scope, withFile({ ...runtime, url: `${indexBase}short.js`, digest: 'abc' }))).toBe(false);
    expect(validSearchManifest(scope, variant({ totalBytes: manifest.totalBytes + 1 }))).toBe(false);
  });

  it('requires the runtime among the files', () => {
    expect(validSearchManifest(scope, variant({ files: manifest.files.slice(1), totalBytes: manifest.totalBytes - manifest.files[0].size }))).toBe(false);
  });
});

describe('verifiedSearchResponse', () => {
  const file = manifest.files[2];
  const body = files[2].body;

  it('returns the body once its size and digest match', async () => {
    const verified = await verifiedSearchResponse(new Response(body, { headers: { 'content-type': 'text/plain' } }), file, 1000);
    expect(await verified.text()).toBe(body);
    expect(verified.headers.get('content-type')).toBe('text/plain');
  });

  it('rejects short, long, and tampered bodies as integrity failures', async () => {
    await expect(verifiedSearchResponse(new Response(body.slice(1)), file, 1000)).rejects.toThrow('integrity');
    await expect(verifiedSearchResponse(new Response(`${body}!`), file, 1000)).rejects.toThrow('integrity');
    const tampered = body.replace('complete', 'COMPLETE');
    expect(await digest(tampered)).not.toBe(file.digest);
    await expect(verifiedSearchResponse(new Response(tampered), file, 1000)).rejects.toThrow('integrity');
  });

  it('treats error statuses and stalled bodies as network failures', async () => {
    await expect(verifiedSearchResponse(new Response(body, { status: 500 }), file, 1000)).rejects.toThrow('network');
    const stalled = new ReadableStream<Uint8Array>({ start() {} });
    await expect(verifiedSearchResponse(new Response(stalled), file, 10)).rejects.toThrow('network');
  });
});

describe('completeSearch', () => {
  it('is false until every manifest file is stored in the versioned cache', async () => {
    const { ctx, storage } = harness();
    expect(await completeSearch(ctx, manifest)).toBe(false);
    const cache = await storage.open(ctx.names.offlineSearchPrefix + version);
    for (const file of files.slice(0, 2)) await cache.put(`${scope}${file.url}`, new Response(file.body));
    expect(await completeSearch(ctx, manifest)).toBe(false);
    await cache.put(`${scope}${files[2].url}`, new Response(files[2].body));
    expect(await completeSearch(ctx, manifest)).toBe(true);
    expect(await completeSearch(ctx, { ...manifest, version: 'nope' })).toBe(false);
  });
});

describe('index updates', () => {
  it('reuses byte-identical files from the previous index, keeps it until commit, then retains one predecessor', async () => {
    const { ctx, requests, state, storage } = harness();
    await configureOfflineSearch(ctx, true);
    expect(ctx.state.searchStatus.phase).toBe('ready');
    const fetched = requests.length;
    // A stale third index is dropped as soon as the update starts.
    await storage.open(`${ctx.names.offlineSearchPrefix}${'d'.repeat(64)}`);
    const next = 'c'.repeat(64);
    const nextBase = `pagefind/${next}/`;
    const nextFiles = manifest.files.map((file) => ({ ...file, url: file.url.replace(indexBase, nextBase) }));
    const nextManifest = { version: next, base: nextBase, files: nextFiles, totalBytes: manifest.totalBytes };
    ctx.config.search_manifest = { version: next, base: nextBase };
    state.network = async (url) =>
      url.endsWith('search-manifest.json') ? Response.json(nextManifest) : new Response(`unexpected ${url}`);
    await configureOfflineSearch(ctx, true);
    expect(ctx.state.searchStatus).toMatchObject({ phase: 'ready', activeVersion: next, base: nextBase });
    expect(requests.slice(fetched)).toEqual([`${scope}${nextBase}search-manifest.json`]);
    const names = await storage.keys();
    expect(names.filter((name) => name.startsWith(ctx.names.offlineSearchPrefix)).sort()).toEqual(
      [ctx.names.offlineSearchPrefix + version, ctx.names.offlineSearchPrefix + next].sort(),
    );
    expect(await completeSearch(ctx, nextManifest)).toBe(true);
  });
});
