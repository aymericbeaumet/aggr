import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createPlayback, endsAt, followProvider } from '../../themes/default/static/media.js';

class Audio extends EventTarget {
  currentTime=0; duration=120; playbackRate=1; volume=1; muted=false; paused=true; ended=false; error=null;
  play(){this.paused=false;this.dispatchEvent(new Event('play'));return Promise.resolve();}
  pause(){this.paused=true;this.dispatchEvent(new Event('pause'));}
  load(){}
}
const flush=()=>new Promise(resolve=>queueMicrotask(resolve));

test('audio controls preserve skip, seek, speed, mute restoration and paused remaining time',async()=>{
  const audio=new Audio(), snapshots=[];
  const player=createPlayback(audio,state=>snapshots.push(state)); await flush();
  player.seek(.5); player.skip(-15); player.speed(1.5);
  assert.equal(audio.currentTime,45);assert.equal(audio.playbackRate,1.5);
  assert.equal(snapshots.at(-1).remaining,50);assert.equal(snapshots.at(-1).endsAt,undefined);
  player.volume(.4);player.mute();assert.equal(audio.volume,0);assert.equal(audio.muted,true);
  player.mute();assert.equal(audio.volume,.4);assert.equal(audio.muted,false);
  player.toggle();audio.dispatchEvent(new Event('playing'));
  assert.ok(snapshots.at(-1).endsAt>Date.now());
  const count=snapshots.length;player.dispose();audio.dispatchEvent(new Event('timeupdate'));
  assert.equal(snapshots.length,count);assert.equal(audio.paused,true);
});

test('playback errors and device-controlled volume remain recoverable',async()=>{
  const audio=new Audio(), snapshots=[];
  audio.play=()=>Promise.reject(Error('blocked'));
  Object.defineProperty(audio,'volume',{get:()=>1,set:()=>{}});
  const player=createPlayback(audio,state=>snapshots.push(state)); await flush();
  player.volume(.4);assert.equal(snapshots.at(-1).volumeAdjustable,false);
  player.toggle();await flush();assert.equal(snapshots.at(-1).phase,'error');
  player.dispose();
});

test('timing suppresses completion for buffering, paused, ended and non-finite streams',()=>{
  for(const phase of ['paused','loading','ended']) assert.equal(endsAt({phase,duration:120,position:60,speed:2},1000).endsAt,undefined);
  assert.deepEqual(endsAt({phase:'playing',duration:120,position:60,speed:2},1000),{remaining:30,endsAt:31000});
  assert.deepEqual(endsAt({phase:'playing',duration:Infinity,position:60,speed:2}),{});
});

test('provider telemetry requires the exact frame and origin and stops when its page ends',()=>{
  globalThis.window=new EventTarget();
  const controller=new AbortController(), snapshots=[], sent=[];
  const source={postMessage:message=>sent.push(message)}, frame={contentWindow:source};
  const subscribe=followProvider(frame,'youtube','https://www.youtube-nocookie.com',state=>snapshots.push(state),120,controller.signal);
  subscribe();assert.ok(sent.length);
  const message=(origin,sourceWindow,info)=>{
    const event=new Event('message');Object.assign(event,{origin,source:sourceWindow,data:{event:'infoDelivery',info}});window.dispatchEvent(event);
  };
  const info={currentTime:30,duration:120,playbackRate:1.5,playerState:1,videoData:{isLive:false}};
  message('https://attacker.invalid',source,info);message('https://www.youtube-nocookie.com',{},info);assert.equal(snapshots.length,0);
  message('https://www.youtube-nocookie.com',source,info);assert.equal(snapshots.at(-1).phase,'playing');
  const count=snapshots.length;controller.abort();message('https://www.youtube-nocookie.com',source,info);assert.equal(snapshots.length,count);
});
