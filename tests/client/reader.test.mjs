import assert from 'node:assert/strict';
import { test } from 'node:test';
import { encodeSelection,decodeSelection,pullState,offlineSummary } from '../../themes/default/static/reader.js';

test('shared word ranges retain the baseline fragment format and reject invalid or unsafe offsets',()=>{
  assert.equal(encodeSelection(3,7),'Myw3');assert.deepEqual(decodeSelection('Myw3'),[3,7]);
  for(const range of ['2,2','3,2','-1,2','0,3,4','0,9007199254740992','NaN,1']) assert.equal(decodeSelection(btoa(range)),null);
  assert.equal(decodeSelection('not-a-token'),null);
});

test('pull refresh arms only a downward tracked gesture after the threshold',()=>{
  assert.deepEqual(pullState('idle',0,120),{phase:'idle',distance:0});
  assert.equal(pullState('tracking',10,5).phase,'idle');
  assert.equal(pullState('tracking',0,5).phase,'tracking');
  assert.equal(pullState('tracking',0,40).phase,'pulling');
  assert.deepEqual(pullState('pulling',0,200),{phase:'armed',distance:72});
  assert.equal(pullState('armed',100,20).phase,'idle');
});

test('offline status distinguishes complete articles, partial downloads, and complete search',()=>{
  const status={requested:2,total:2,saved:[{url:'item/',title:'One'}],failed:1,downloading:false};
  assert.match(offlineSummary(status,true),/1 of 2 articles/);assert.match(offlineSummary(status,true),/incomplete/);
  assert.doesNotMatch(offlineSummary({...status,search:{phase:'downloading',downloadedFiles:1,totalFiles:3}},true),/search is available/);
  assert.match(offlineSummary({...status,search:{phase:'ready'}},true),/Full archive search is available/);
  assert.match(offlineSummary({...status,requested:0},true),/downloads are off/);
});
