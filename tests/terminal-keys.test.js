import test from 'node:test';
import assert from 'node:assert/strict';
import {installTerminalKeys} from '../web/terminal-keys.js';

function harness() {
  const queued = [], raw = [];
  let owner = {session:'initial'};
  const keys = installTerminalKeys({
    getOwner:()=>owner,
    send(event,receipt,permit=()=>true) {queued.push({event,receipt,permit});return true;},
    sendText:(text,attachment)=>raw.push({text,attachment}),
  });
  return {keys,queued,raw,owner:()=>owner,replaceOwner() {owner={session:'next'};}};
}
const a = {key:'a',code:'KeyA'};
const b = {key:'b',code:'KeyB'};

test('a release reserves its original queue order and waits for the press receipt',()=>{
  const h=harness();
  assert.equal(h.keys.down(a,true),false);
  h.keys.text('a',h.owner());
  h.keys.up(a);
  h.keys.down(b,false);
  assert.deepEqual(h.queued.map(({event})=>[event.phase,event.code]),[
    ['press','KeyA'],['release','KeyA'],['press','KeyB'],
  ]);
  assert.equal(h.queued[0].event.text,'a');
  assert.equal(h.queued[1].permit(),false);
  h.queued[0].receipt(true);
  assert.equal(h.queued[1].permit(),true);
});

test('legacy keyups skip IPC and a release cannot follow a different pane',()=>{
  const h=harness();
  h.keys.down(a,false);
  h.keys.up(a);
  h.queued[0].receipt(false);
  assert.equal(h.queued[1].permit(),false);
  h.keys.down(b,false);
  h.keys.up(b);
  h.queued[2].receipt(true);
  assert.equal(h.queued[3].permit(),true);
  h.replaceOwner();
  assert.equal(h.queued[3].permit(),false);
});

test('a mismatched or retired keydown cannot become associated text',()=>{
  const h=harness();
  h.keys.down(a,true);
  h.keys.text('你',h.owner());
  h.keys.down(b,true);
  h.keys.clearText();
  h.keys.text('b',h.owner());
  h.keys.down(a,true);
  h.keys.reset();
  h.keys.up(a);
  assert.deepEqual(h.raw.map(x=>x.text),['你','b']);
  assert.deepEqual(h.queued,[]);
});

test('modifier releases include the effect of the remaining side',()=>{
  const h=harness();
  const left={key:'Shift',code:'ShiftLeft',shiftKey:false};
  const right={key:'Shift',code:'ShiftRight',shiftKey:true};
  h.keys.down(left,false);
  h.keys.down(right,false);
  h.queued[0].receipt(true);
  h.queued[1].receipt(true);
  h.keys.up(right);
  h.keys.up({...left,shiftKey:true});
  assert.deepEqual(h.queued.map(({event})=>[event.phase,event.shift]),[
    ['press',true],['press',true],['release',true],['release',false],
  ]);
});
