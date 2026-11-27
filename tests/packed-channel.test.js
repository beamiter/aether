import test from 'node:test';
import assert from 'node:assert/strict';
import {acceptChannelFrame, acceptHistoryProjection, CHANNEL_FRAME_VERSION, CHANNEL_PACKED_VERSION} from '../web/frames-v4.js';
import {packedRow} from './fixtures/packed-channel.js';

const owner = {session: 'packed', epoch: 1, generation: 1};
const stamp = {session: owner.session, epoch:'1', resource_generation:'1', revision:'1', layout:'1',
  screen:'primary', cursor_row:0, cursor_col:4, cursor_shape:'beam', cursor_visible:true, pending_wrap:false};
const style = {foreground:'#abcdef',background:'#222222',style:0};
const cell = {text:'X',width:1,...style};
const cluster = {...cell,text:'e\u0301',underline:'curly',underline_color:'#987654',hyperlink_id:23};
const wide = {...cell,text:'界',width:2};
const half = {...cell,text:' ',width:0};
const image = {id:'3',width:1,height:1,rows:1,columns:1,rgba:'/wAA/w=='};
const linked = {id:23,target:'https://example.test/packed'};
const row = {wrapped:false,cells:[cell, {...cell,text:'𐐀'},cluster,wide,half],
  image_slices:[{column:0,id:image.id,slice:0}]};
const plain = {wrapped:false,cells:Array(5).fill(cell)};
const snapshot = {kind:'snapshot',stamp:{...stamp,version:CHANNEL_PACKED_VERSION},
  cols:5,rows:[packedRow(row),packedRow(plain)],images:[image],hyperlinks:[linked]};
const accept = (previous,value) => acceptChannelFrame(previous,JSON.stringify(value),
  {...owner,frameVersion:value.stamp.version});

test('packed rows expand to the same glyphs, widths, colors and resources as v6 and can move',()=>{
  const legacy = accept(null,{...snapshot,stamp:{...stamp,version:CHANNEL_FRAME_VERSION},rows:[row,plain]});
  const compact = accept(null,snapshot);
  assert.deepEqual(compact.rows,legacy.rows);
  assert.deepEqual(compact.hyperlinks,legacy.hyperlinks);
  assert.deepEqual(compact.images,legacy.images);
  assert.deepEqual(accept(null,{...snapshot,rows:[row,packedRow(plain)]}),compact);
  const moved = accept(compact,{kind:'delta',stamp:{...snapshot.stamp,revision:'2'},base_revision:'1',
    rows:[{index:0,row:packedRow(plain)}],moves:[{from:0,to:1,count:1}]});
  assert.equal(moved.rows[1],compact.rows[0]);
  assert.equal(moved.hyperlinks[23],linked.target);
  assert.equal(moved.images[image.id],compact.images[image.id]);
});

test('packed rows enforce width, style references, grapheme bytes and scalar validity',()=>{
  const paint = packedRow(plain);
  const check = change => accept(null,{...snapshot,rows:[{...paint,...change}],hyperlinks:[],images:[]});
  assert.throws(()=>check({runs:[[0,'abcdef']]}),/row width/);
  assert.throws(()=>check({runs:[[0,'AB'],[1,'CDE']]}),/run/);
  assert.throws(()=>check({runs:[[0,'ABCD'],[0,'界',2]]}),/wide span/);
  assert.throws(()=>check({runs:[[0,'\ud800ABCD']]}),/run/);
  assert.throws(()=>check({runs:[[0,'X'.repeat(65),1],[0,'ABCD']]}),/run/);
  assert.throws(()=>check({styles:[style,{...style,foreground:'#ffffff'}]}),/styles/);
  assert.throws(()=>check({columns:4}),/row/);
  assert.throws(()=>check({cells:plain.cells}),/mixed/);
  assert.throws(()=>accept(null,{...snapshot,stamp:{...stamp,version:CHANNEL_FRAME_VERSION}}),/row/);
});

test('packed history keeps UTF-16 offsets, wrap, and short-row padding',()=>{
  const first = {wrapped:true,cells:[{...cell,text:'𐐀'},cluster]};
  const second = {wrapped:false,cells:[wide,half]};
  const raw = [{source:{row:'12',column:0},paint:first,text_offsets:[0,2,4]},
    {source:{row:'13',column:0},paint:second,text_offsets:[4,5,5]}];
  const legacy = {version:5,owner,revision:'8',total_rows:2,offset:0,cols:5,text:'𐐀e\u0301界',rows:raw,hyperlinks:[linked]};
  const current = {...legacy,version:CHANNEL_PACKED_VERSION,rows:raw.map(entry=>({...entry,paint:packedRow(entry.paint)}))};
  assert.deepEqual(acceptHistoryProjection(JSON.stringify(current),owner,false,CHANNEL_PACKED_VERSION),
    acceptHistoryProjection(JSON.stringify(legacy),owner));
  assert.throws(()=>acceptHistoryProjection(JSON.stringify(legacy),owner,false,CHANNEL_PACKED_VERSION),/foreign/);
});
