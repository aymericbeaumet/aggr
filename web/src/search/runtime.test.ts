import { afterEach, describe, expect, it, vi } from 'vitest';
import { SearchEngine } from './engine';
import { parseQuery } from './query';
import type { PagefindAPI, PagefindModule, ResultData } from './types';

afterEach(()=>vi.restoreAllMocks());
function fixture() {
  let version='v1';
  vi.spyOn(globalThis,'fetch').mockImplementation(async()=>new Response(JSON.stringify({version,base:`pagefind/${version}/`,docs:1,facets:{source:[],category:[],tag:[],'published-day':[]}})));
  const instances: {base:string;api:PagefindAPI;destroy:ReturnType<typeof vi.fn>;data:ReturnType<typeof vi.fn>}[]=[];
  const modules=new Map<string,PagefindModule>();
  const engine=new SearchEngine('https://reader.test/',async url=>{
    if(!modules.has(url)) modules.set(url,{createInstance(options){
      const destroy=vi.fn(async()=>{}),data=vi.fn(async()=>({url:'/article/',meta:{}}));
      const api:PagefindAPI={init:async()=>{},options:async()=>{},destroy,preload:async()=>{},filters:async()=>({}),search:async()=>({results:[{id:'article',data}]})};
      instances.push({base:String(options.basePath),api,destroy,data});
      return api;
    }});
    return modules.get(url)!;
  });
  return {engine,instances,serve:(next:string)=>{version=next;engine.markStale();}};
}
const query=parseQuery('article');

describe('leased Pagefind runtimes',()=>{
  it('destroys evicted instances and releases all retained instances at application disposal',async()=>{
    const {engine,instances,serve}=fixture();
    await engine.search(query,1,25);
    serve('v2');await engine.search(query,1,25);
    serve('v3');await engine.search(query,1,25);
    expect(instances.map(instance=>instance.base)).toEqual(['/pagefind/v1/','/pagefind/v2/','/pagefind/v3/']);
    expect(instances.map(instance=>instance.destroy.mock.calls.length)).toEqual([1,0,0]);
    await engine.dispose();
    expect(instances.map(instance=>instance.destroy.mock.calls.length)).toEqual([1,1,1]);
    await engine.dispose();
    expect(instances[2].destroy).toHaveBeenCalledTimes(1);
  });
  it('keeps an evicted index alive through pending hydration and isolates same-version reopening',async()=>{
    const {engine,instances,serve}=fixture();
    await engine.search(query,1,25);
    let finish!: (data:ResultData)=>void;
    instances[0].data.mockImplementation(()=>new Promise(resolve=>{finish=resolve;}));
    const pending=engine.search(parseQuery('other query'),1,25);
    await vi.waitFor(()=>expect(finish).toBeTypeOf('function'));
    serve('v2');await engine.search(query,1,25);
    serve('v3');await engine.search(query,1,25);
    expect(instances[0].destroy).not.toHaveBeenCalled();
    serve('v1');await engine.search(query,1,25);
    expect(instances[3].base).toBe('/pagefind/v1/');
    finish({url:'/article/',meta:{title:'Completed old query'}});
    expect((await pending).results[0].meta.title).toBe('Completed old query');
    expect(instances[0].destroy).toHaveBeenCalledTimes(1);
    expect(instances[3].destroy).not.toHaveBeenCalled();
    await engine.dispose();
    expect(instances[3].destroy).toHaveBeenCalledTimes(1);
  });
  it('waits for active queries during disposal without destroying their worker early',async()=>{
    const {engine,instances}=fixture();
    await engine.search(query,1,25);
    let finish!: (data:ResultData)=>void;
    instances[0].data.mockImplementation(()=>new Promise(resolve=>{finish=resolve;}));
    const pending=engine.search(parseQuery('pending query'),1,25);
    await vi.waitFor(()=>expect(finish).toBeTypeOf('function'));
    let disposed=false;
    const disposal=engine.dispose().then(()=>{disposed=true;});
    await Promise.resolve();
    expect(disposed).toBe(false);
    expect(instances[0].destroy).not.toHaveBeenCalled();
    await expect(engine.load()).rejects.toThrow(/disposed/);
    finish({url:'/article/',meta:{}});
    await pending;await disposal;
    expect(instances[0].destroy).toHaveBeenCalledTimes(1);
  });
  it('keeps cancelled facet work leased until its pending worker request finishes',async()=>{
    const {engine,instances,serve}=fixture();
    await engine.search(query,1,25);
    let finish!: ()=>void;
    instances[0].api.filters=()=>new Promise(resolve=>{finish=()=>resolve({});});
    const cancellation=new AbortController();
    const pending=engine.facetCounts(query,'source',cancellation.signal);
    await vi.waitFor(()=>expect(finish).toBeTypeOf('function'));
    serve('v2');await engine.search(query,1,25);
    serve('v3');await engine.search(query,1,25);
    cancellation.abort();
    expect(instances[0].destroy).not.toHaveBeenCalled();
    finish();
    await expect(pending).rejects.toThrow();
    expect(instances[0].destroy).toHaveBeenCalledTimes(1);
    await engine.dispose();
  });
  it('destroys a failed initialization and allows the same index to retry with a fresh instance',async()=>{
    vi.spyOn(globalThis,'fetch').mockImplementation(async()=>new Response(JSON.stringify({version:'v1',base:'pagefind/v1/',docs:0,facets:{source:[],category:[],tag:[],'published-day':[]}})));
    const destroys:ReturnType<typeof vi.fn>[]=[];
    const engine=new SearchEngine('https://reader.test/',async()=>({createInstance(){
      const attempt=destroys.length,destroy=vi.fn(async()=>{});destroys.push(destroy);
      return {init:async()=>{if(attempt===0)throw Error('temporary worker failure');},destroy,options:async()=>{},preload:async()=>{},filters:async()=>({}),search:async()=>({results:[]})};
    }}));
    await expect(engine.search(query,1,25)).rejects.toThrow('temporary worker failure');
    expect(destroys[0]).toHaveBeenCalledTimes(1);
    expect((await engine.search(query,1,25)).total).toBe(0);
    await engine.dispose();
    expect(destroys.map(destroy=>destroy.mock.calls.length)).toEqual([1,1]);
  });
});
