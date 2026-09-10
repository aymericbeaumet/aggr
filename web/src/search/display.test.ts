import { afterEach, describe, expect, it, vi } from 'vitest';
import { displayData, placeholderBackground, resultExcerptParts } from './display';
import type { ResultData } from './types';

const opaque = (value:object) => Buffer.from(JSON.stringify(value),'utf8').toString('hex');
it('only accepts bounded inline PNG placeholders from search metadata', () => {
  expect(placeholderBackground('data:image/png;base64,AA==')).toBe('url("data:image/png;base64,AA==")');
  for (const value of [undefined, 'https://publisher.example/image.png', 'data:image/svg+xml,<svg/>', 'data:image/png;base64,AA\");color:red', 'data:image/png;base64,' + 'A'.repeat(8192)]) {
    expect(placeholderBackground(value)).toBeUndefined();
  }
});
afterEach(() => {vi.restoreAllMocks();vi.unstubAllGlobals();});

describe('immutable search display preparation', () => {
  it('decodes metadata once per result object across rendering and offline checks', () => {
    const entry={url:'/article/',meta:{aggr_display:opaque({source_display:'Publisher',excerpt:'Preview'})}};
    const parse=vi.spyOn(JSON,'parse');
    const first=displayData(entry);
    expect(first.source_display).toBe('Publisher');
    for(let i=0;i<20;i++) expect(displayData(entry)).toBe(first);
    expect(parse).toHaveBeenCalledTimes(1);
    expect(displayData({...entry})).toEqual(first);
    expect(parse).toHaveBeenCalledTimes(2);
  });
  it('also caches invalid display data while allowing a replacement result to recover', () => {
    const entry={url:'/article/',meta:{aggr_display:'7b'}};
    const parse=vi.spyOn(JSON,'parse');
    expect(displayData(entry)).toEqual({});
    expect(displayData(entry)).toEqual({});
    expect(parse).toHaveBeenCalledTimes(1);
    const replacement={...entry,meta:{aggr_display:opaque({excerpt:'Fixed'})}};
    expect(displayData(replacement).excerpt).toBe('Fixed');
  });
  it('parses highlighted excerpts once per result, without confusing other queries for the same URL', () => {
    const parse=vi.fn((html:string) => {
      const nodes=[{textContent:html,parentElement:{closest:(selector:string)=>selector==='mark'?{}:null}}];
      let index=-1;
      return {body:{},createTreeWalker:()=>({nextNode:()=>!!nodes[++index],get currentNode(){return nodes[index];}})};
    });
    vi.stubGlobal('DOMParser',class {parseFromString=parse;});
    vi.stubGlobal('NodeFilter',{SHOW_TEXT:4});
    const first:ResultData={url:'/article/',meta:{},excerpt:'first query'};
    const second:ResultData={...first,excerpt:'second query'};
    const parts=resultExcerptParts(first);
    expect(parts).toEqual([{text:'first query',highlight:true}]);
    for(let i=0;i<20;i++) expect(resultExcerptParts(first)).toBe(parts);
    expect(resultExcerptParts(second)).toEqual([{text:'second query',highlight:true}]);
    expect(parse).toHaveBeenCalledTimes(2);
  });
  it('reuses a plain fallback without creating an HTML parser', () => {
    const parser=vi.fn();vi.stubGlobal('DOMParser',parser);
    const entry={url:'/article/',meta:{aggr_display:opaque({excerpt:'Plain preview'})}};
    const parts=resultExcerptParts(entry);
    expect(parts).toEqual([{text:'Plain preview',highlight:false}]);
    expect(resultExcerptParts(entry)).toBe(parts);
    expect(parser).not.toHaveBeenCalled();
  });
});
