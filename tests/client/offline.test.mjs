import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

const scope = 'https://reader.invalid/archive/';
const version = 'a'.repeat(64), indexBase = `pagefind/${version}/`;
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const files = [
  {url:indexBase + 'pagefind.js', body:'export const index = 1;'},
  {url:indexBase + 'search-catalog.json', body:JSON.stringify({version,base:indexBase,docs:2,facets:{}})},
  {url:indexBase + 'fragment.pf_fragment', body:'complete searchable article'},
];
const manifest = {version,base:indexBase,files:files.map(file=>({url:file.url,size:Buffer.byteLength(file.body),digest:digest(file.body)})),totalBytes:files.reduce((total,file)=>total+Buffer.byteLength(file.body),0)};
const catalog = [
  {url:'items/new/',title:'Newest',resources:[{url:'items/new/',revision:'new'},{url:'assets/images/shared.webp',revision:'image'},{url:'assets/images/large.webp',revision:'large'}]},
  {url:'items/older/',title:'Older',resources:[{url:'items/older/',revision:'older'},{url:'assets/images/shared.webp',revision:'image'}]},
];

function harness() {
  const buckets = new Map(), handlers = new Map(), messages = [], requests = [];
  const state = {offline:false, quota:()=>false, network:undefined};
  const key = request => new URL(typeof request === 'string' ? request : request.url,scope).href;
  const storage = {
    keys:async()=>[...buckets.keys()], delete:async name=>buckets.delete(name),
    open:async name=>{
      if (!buckets.has(name)) buckets.set(name,new Map());
      const entries = buckets.get(name);
      return {
        put:async(request,response)=>{ const url=key(request); if(state.quota(name,url))throw new DOMException('quota exceeded','QuotaExceededError'); entries.set(url,response.clone()); },
        delete:async request=>entries.delete(key(request)),
        keys:async()=>[...entries.keys()].map(url=>new Request(url)),
        match:async(request,options)=>{
          const wanted = new URL(key(request));
          for (const [url,response] of entries) if (url===wanted.href || options?.ignoreSearch && new URL(url).pathname===wanted.pathname) return response.clone();
        },
      };
    },
    match:async(request,options)=>{ for(const name of buckets.keys()) {const result=await(await storage.open(name)).match(request,options);if(result)return result;} },
  };
  const self = {
    location:new URL(scope+'sw.js'), registration:{scope},
    clients:{claim:async()=>{},matchAll:async()=>[{postMessage:message=>messages.push(message)}]},
    skipWaiting:async()=>{}, addEventListener:(name,listener)=>handlers.set(name,[...(handlers.get(name)||[]),listener]),
  };
  const substitutions = {version:'build', 'build.app_version':'app', 'build.content_version':'content', precache:[{url:'',required:true}],offline_count:0,offline_catalog:catalog,search_manifest:{version,base:indexBase}};
  const source = readFileSync(new URL('../../themes/default/templates/sw.js',import.meta.url),'utf8').replace(/\{\{ ([\w.]+) \| json \}\}/g,(_,name)=>JSON.stringify(substitutions[name]));
  assert.doesNotMatch(source,/\{\{/);
  const fetch = async(request,options)=>{
    const url=key(request); requests.push(url);
    if(state.offline)throw Error('offline');
    if(state.network)return state.network(url,options);
    if(url.endsWith('search-manifest.json'))return Response.json(manifest);
    const file=files.find(file=>key(file.url)===url);
    return new Response(file?.body || url);
  };
  const api = new Function('self','caches','fetch',source+'\nreturn {saveOfflineArticles,configureOffline,configureOfflineSearch,readOfflineStatus,pageResponse,assetResponse,OFFLINE_PAGES,OFFLINE_SETTINGS,OFFLINE_SEARCH_PREFIX,PAGES,ASSETS,searchStatus:()=>searchStatus,completeSearch,verifiedSearchResponse,fetchEntry,setTimeout:ms=>PRECACHE_TIMEOUT=ms};')(self,storage,fetch);
  return {api,state,storage,buckets,messages,requests,handlers};
}

test('offline articles require the page and every retained rendition, sharing resources across items',async()=>{
  const {api,requests,storage,state}=harness();
  const result=await api.saveOfflineArticles(2);
  assert.equal(result.saved.length,2); assert.equal(result.failed,0);
  assert.equal(requests.filter(url=>url.endsWith('shared.webp')).length,1);
  const protectedPages=await storage.open(api.OFFLINE_PAGES);
  assert.ok(await protectedPages.match(scope+'assets/images/large.webp'));
  await api.configureOffline(2);
  await protectedPages.delete(scope+'assets/images/large.webp');
  const partial=await api.readOfflineStatus();
  assert.deepEqual(partial.saved.map(item=>item.title),['Older'],'one missing image makes its article incomplete');
  state.offline=true;
  assert.equal(await(await api.pageResponse(new Request(scope+'items/older/'))).text(),scope+'items/older/');
});

test('quota failures preserve previous complete downloads and never advertise an incomplete replacement',async()=>{
  const {api,state,storage}=harness();
  await api.saveOfflineArticles(2);
  await (await storage.open(api.OFFLINE_PAGES)).delete(scope+'assets/images/large.webp');
  state.quota=(name,url)=>name===api.OFFLINE_PAGES && url.endsWith('large.webp');
  const result=await api.saveOfflineArticles(2);
  assert.equal(result.failed,1);
  assert.deepEqual(result.saved.map(item=>item.title),['Older']);
  assert.ok(await(await storage.open(api.OFFLINE_PAGES)).match(scope+'items/older/'));
});

test('offline search is committed only after the complete manifest passes size and digest verification',async()=>{
  const {api,state,storage}=harness();
  state.network=async url=>url.endsWith('search-manifest.json')?Response.json(manifest):new Response('corrupt');
  await api.configureOfflineSearch(true);
  assert.equal(api.searchStatus().phase,'error'); assert.equal(api.searchStatus().activeVersion,null);
  assert.equal(await api.completeSearch(manifest),false);
  state.network=undefined;
  await api.configureOfflineSearch(true);
  assert.equal(api.searchStatus().phase,'ready'); assert.equal(api.searchStatus().activeVersion,version);
  assert.equal(await api.completeSearch(manifest),true);
  const index=await storage.open(api.OFFLINE_SEARCH_PREFIX+version);
  assert.ok(await index.match(scope+indexBase+'search-catalog.json'));
  await api.configureOffline(0);
  assert.equal(api.searchStatus().phase,'disabled');
  assert.equal((await api.readOfflineStatus()).saved.length,0);
});

test('disabling while downloads are in flight cannot resurrect saved articles or search readiness',async()=>{
  const {api,state}=harness();
  state.network=async(_url,options)=>new Promise((_,reject)=>options.signal.addEventListener('abort',()=>reject(new DOMException('aborted','AbortError')),{once:true}));
  const pending=api.configureOffline(2);
  await new Promise(resolve=>setImmediate(resolve));
  await api.configureOffline(0); await pending;
  const status=await api.readOfflineStatus();
  assert.equal(status.requested,0); assert.equal(status.saved.length,0); assert.equal(status.search.phase,'disabled');
});

test('required shell failure rejects installation, and quota never hides a valid network response',async()=>{
  const {api,state,handlers}=harness();
  state.offline=true;
  const installs=[]; for(const listener of handlers.get('install'))listener({waitUntil:promise=>installs.push(promise)});
  await assert.rejects(Promise.all(installs));
  state.offline=false;state.quota=()=>true;
  assert.equal((await api.pageResponse(new Request(scope+'items/fresh/'))).status,200);
  assert.equal((await api.assetResponse(new Request(scope+'assets/example.js'),api.ASSETS,256)).status,200);
});


test('corrupted persisted metadata and bodyless index responses cannot establish readiness',async()=>{
  const {api,storage}=harness();
  const settings=await storage.open(api.OFFLINE_SETTINGS);
  await settings.put(scope+'__offline_count',new Response('2'));
  await settings.put(scope+'__offline_articles',Response.json([null,{url:'items/new/',title:'New',resources:[null]}]));
  await settings.put(scope+'__offline_search',Response.json({version,files:null}));
  const status=await api.readOfflineStatus();
  assert.equal(status.saved.length,0);assert.equal(status.search.activeVersion,null);
  await assert.rejects(api.verifiedSearchResponse(new Response(null),manifest.files[0]),/network/);
});
