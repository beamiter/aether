import test from 'node:test';
import assert from 'node:assert/strict';
import {acceptChannelFrame, acceptHistoryProjection, CELL_CONCEALED, CELL_OVERLINE,
  CHANNEL_FRAME_VERSION, CHANNEL_PACKED_VERSION, CHANNEL_RENDITION_VERSION} from '../web/frames-v4.js';
import {acceptFull, paint} from '../web/frames.js';
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

test('changing a packed continuation background restores its complete wide glyph and cursor',()=>{
  const wideRow = {wrapped:false,cells:[cell,cell,cell,wide,half]};
  const initial = accept(null,{...snapshot,rows:[packedRow(wideRow)],images:[],hyperlinks:[]});
  const changed = {...wideRow,cells:[...wideRow.cells.slice(0,-1),{...half,background:'#445566'}]};
  const update = (base, revision, row) => accept(base,{kind:'delta',base_revision:base.revision,
    stamp:{...snapshot.stamp,revision},rows:[{index:0,row:packedRow(row)}]});
  const next = update(initial,'2',changed);
  const clears = [], glyphs = [], fills = [];
  const context = {setTransform(){},save(){},beginPath(){},rect(){},clip(){},restore(){},
    clearRect(...rect){clears.push(rect);},fillText(...glyph){glyphs.push(glyph);},
    fillRect(...rect){fills.push([this.fillStyle,...rect]);}};
  const canvas = {style:{},getContext:()=>context};
  paint(canvas,initial,1);
  clears.length = glyphs.length = fills.length = 0;
  paint(canvas,next,1);
  assert.deepEqual(clears,[[36,0,24,24]]);
  assert.deepEqual(glyphs,[['界',36,2]]);
  assert.deepEqual(fills,[['#222222',36,0,12,24],['#445566',48,0,12,24],['#9ac6ff',48,0,2,24]]);
  clears.length = glyphs.length = fills.length = 0;
  const rewrapped = update(next,'3',{...changed,wrapped:true});
  assert.equal(rewrapped.rows[0].wrapped,true);
  paint(canvas,rewrapped,1);
  assert.deepEqual([clears,glyphs,fills],[[],[],[]]);
});

test('version 8 suppresses concealed glyphs and decorations and extends an overline across a wide cell',()=>{
  const rendition = {wrapped:false,cells:[
    {...wide,text:'🧪',style:4|8|128,underline:'double',hyperlink_id:23},
    half, {...wide,style:128}, half, cell,
  ]};
  const current = {...snapshot,images:[],stamp:{...stamp,version:CHANNEL_RENDITION_VERSION,cursor_visible:false}};
  const frame = accept(null,{...current,rows:[packedRow(rendition)]});
  assert.deepEqual(frame.rows,accept(null,{...current,rows:[rendition]}).rows);
  assert.equal(frame.rows[0][0].style,CELL_CONCEALED|CELL_OVERLINE|8);
  assert.equal(frame.rows[0][0].text,'🧪');
  const glyphs = [], fills = [];
  const context = {setTransform(){},save(){},beginPath(){},rect(){},clip(){},restore(){},clearRect(){},
    fillText(...glyph){glyphs.push(glyph);},fillRect(...fill){fills.push(fill);}};
  paint({style:{},getContext:()=>context},frame,1);
  assert.deepEqual(glyphs,[['界',24,2],['X',48,2]]);
  assert.deepEqual(fills.filter(fill=>fill[3]===1),[[24,1,24,1]]);
  const legacy = {...current,stamp:{...current.stamp,version:CHANNEL_PACKED_VERSION},rows:[packedRow(rendition)]};
  assert.throws(()=>accept(null,legacy),/style/);

  const history = acceptHistoryProjection(JSON.stringify({
    version:CHANNEL_RENDITION_VERSION,owner,revision:'2',total_rows:1,offset:0,cols:5,text:'🧪界X',
    rows:[{source:{row:'12',column:0},paint:packedRow(rendition),text_offsets:[0,2,2,3,3,4]}],
    hyperlinks:[linked],
  }),owner,false,CHANNEL_RENDITION_VERSION);
  assert.equal(history.text,'🧪界X');
  assert.deepEqual(history.offsets[0],[0,2,2,3,3,4]);
  assert.equal(history.rows[0][0].style,CELL_CONCEALED|CELL_OVERLINE|8);
});

test('compatibility rendition is optional and keeps the legacy underline flag distinct',()=>{
  const legacy = {version:3,session:owner.session,epoch:owner.epoch,revision:1,layout:1,cols:2,
    cursor_row:0,cursor_col:0,cursor_visible:false,cursor_shape:'block',pending_wrap:false,
    rows:[[{...cell,style:4},{...cell,rendition:3}]]};
  const frame = acceptFull(null,legacy,owner);
  assert.deepEqual(frame.rows[0].map(c=>c.style),[4,CELL_CONCEALED|CELL_OVERLINE]);
  for (const rendition of [-1,4,null,'1']) {
    assert.throws(()=>acceptFull(null,{...legacy,rows:[[cell,{...cell,rendition}]]},owner),/rendition/);
  }
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
