import test from 'node:test';
import assert from 'node:assert/strict';
import {acceptChannelFrame, channelCounter} from '../web/frames-v4.js';
import {paint} from '../web/frames.js';

const owner = {session:'isolated', epoch:7};
const stamp = {version:4, session:owner.session, epoch:'7', revision:'9007199254740992',
  layout:'18446744073709551615', screen:'primary', resource_generation:'7', cursor_row:1, cursor_col:0,
  cursor_shape:'beam', cursor_visible:false, pending_wrap:false};
const cell = {text:' ', width:1, style:0, foreground:'#ffffff', background:'#000000'};
const cluster = { ...cell, text:'👩‍💻', width:2, underline:'double', underline_color:'#aabbcc', hyperlink_id:22 };
const first = {wrapped:true, cells:[cluster,{...cell,width:0}]};
const second = {wrapped:false, cells:[{...cell,text:'e\u0301'},cell]};
const hyperlinks = [{id:22,target:'https://example.test/path'}];
const snapshot = {kind:'snapshot', stamp, cols:2, rows:[first,second], hyperlinks};
const changed = { ...first, cells:[{...cluster, underline:'curly'},first.cells[1]] };
const delta = {kind:'delta', stamp:{ ...stamp, revision:'9007199254740993' }, base_revision:stamp.revision,
  rows:[{index:0,row:changed}], hyperlinks};
const accept = (previous,value) => acceptChannelFrame(previous,JSON.stringify(value),owner);

test('full and exact delta retain the same high-fidelity cell and row, with exact u64 ordering', () => {
  const full = accept(null,snapshot);
  const next = accept(full,delta);
  const equivalent = accept(null,{...snapshot,stamp:delta.stamp,rows:[changed,second]});
  assert.deepEqual(next,equivalent);
  assert.equal(next.rows[1],full.rows[1]);
  assert.notEqual(next.rows[0],full.rows[0]);
  assert.equal(next.rows[0].wrapped,true);
  assert.deepEqual(next.rows[0][0],changed.cells[0]);
  assert.equal(next.rows[1][0].text,'e\u0301');
  assert.equal(next.hyperlinks[22],hyperlinks[0].target);
  assert.equal(channelCounter(next.layout),18446744073709551615n);
  assert.throws(() => accept(next,delta),/stale/);
  assert.throws(() => accept(full,{...delta,base_revision:'9007199254740991'}),/base/);
  assert.throws(() => accept(full,{...delta,stamp:{...delta.stamp,screen:'alternate'}}),/base/);
  const alternate = accept(full,{...snapshot,stamp:{...delta.stamp,screen:'alternate'}});
  assert.equal(alternate.screen,'alternate');
  for (const invalid of [0,1,'0','01','+1','1e2','18446744073709551616','100000000000000000000'])
    assert.throws(() => channelCounter(invalid), /counter/);
});

test('links resolve within the current resource generation and are reclaimed after the last row changes', () => {
  const base = accept(null,snapshot);
  assert.throws(() => accept(null,{...snapshot,hyperlinks:[]}),/cell/);
  assert.throws(() => accept(null,{...snapshot,hyperlinks:[...hyperlinks,{id:23,target:'https://unused.test'}]}),/unreferenced/);
  assert.throws(() => accept(null,{...snapshot,stamp:{...stamp,resource_generation:'8'}}),/foreign/);
  assert.throws(() => accept(null,{...snapshot,hyperlinks:[{id:22,target:'https://user@unsafe.test'}]}),/authority/);
  assert.throws(() => accept(base,{...delta,hyperlinks:[{id:22,target:'https://different.test'}]}),/identity/);
  const {hyperlink_id,...plain} = changed.cells[0];
  const next = accept(base,{...delta,hyperlinks:[],rows:[{index:0,row:{...changed,cells:[plain,first.cells[1]]}}]});
  assert.equal(next.hyperlinks[hyperlink_id],undefined);
  assert.equal(base.hyperlinks[hyperlink_id],hyperlinks[0].target);
});

test('encoded, cluster, geometry, and accumulated text budgets reject before replacing the frame', () => {
  const full = accept(null,snapshot);
  const applyRow = cells => accept(full,{...delta,rows:[{index:0,row:{...first,cells}}]});
  assert.throws(() => applyRow([{...cluster,text:'界'.repeat(22)},first.cells[1]]),/cluster byte limit/);
  assert.throws(() => applyRow([{...cluster,text:'\x1bX'},first.cells[1]]),/invalid Channel cell/);
  assert.throws(() => applyRow([{...cluster,text:'\ud800'},first.cells[1]]),/invalid Channel cell/);
  assert.throws(() => applyRow([cluster,cell]),/wide span/);
  assert.throws(() => accept(full,{...delta,rows:[delta.rows[0],delta.rows[0]]}),/row index/);
  assert.throws(() => acceptChannelFrame(null,'界'.repeat(3 * 1024 * 1024),owner),/frame byte limit/);
  const longRow = {wrapped:false,cells:Array(256).fill({...cell,text:'界'.repeat(20)+'é'})};
  const extra = {...longRow,cells:Array(256).fill({...cell,text:'🪶'.repeat(16)})};
  const base = accept(null,{...snapshot,cols:256,rows:[...Array(32).fill(longRow),extra],hyperlinks:[]});
  assert.throws(() => accept(base,{...delta,rows:[{index:0,row:extra}],hyperlinks:[]}),/text byte limit/);
  assert.equal(full.rows[0][0].underline,'double');
});

test('Canvas draws the supplied underline colour and both lines across the full cluster span', () => {
  const fills = [];
  const context = {setTransform(){},clearRect(){},save(){},beginPath(){},rect(){},clip(){},fillText(){},restore(){},
    fillRect(...rect){fills.push([this.fillStyle,...rect]);}};
  paint({style:{},getContext:()=>context},accept(null,snapshot),1);
  assert.deepEqual(fills.filter(fill=>fill[0]==='#aabbcc'),[
    ['#aabbcc',0,21,24,1], ['#aabbcc',0,23,24,1],
  ]);
});
