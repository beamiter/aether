import test from 'node:test';
import assert from 'node:assert/strict';
import {acceptChannelFrame, acceptHistoryProjection, channelCounter, CHANNEL_FRAME_VERSION,
  CHANNEL_RENDITION_VERSION, CELL_CONCEALED, SSH_DENSE_FRAME_VERSION} from '../web/frames-v4.js';
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

test('dense rows preserve decorated wide glyphs and validate every cell boundary', () => {
  const bytes = Buffer.from([1,2,3,4,5,6,4,2,3, 10,11,12,4,5,6,0,0,1]);
  const row = {wrapped:true,columns:2,cell_bytes:bytes.toString('base64'),text:'🙂\u0301 ',
    attributes:[{column:0,underline:'curly',underline_color:'#070809',hyperlink_id:22}]};
  const denseSnapshot = {...snapshot,stamp:{...stamp,version:SSH_DENSE_FRAME_VERSION},rows:[row,second]};
  const frame = accept(null,denseSnapshot);
  assert.deepEqual(frame.rows[0][0], {
    text:'🙂\u0301',width:2,style:CELL_CONCEALED,foreground:'#010203',background:'#040506',
    underline:'curly',underline_color:'#070809',hyperlink_id:22,
  });
  assert.equal(frame.rows[0][1].width,0);
  assert.equal(frame.rows[0].textBytes,7);
  assert.equal(frame.rows[0].wrapped,true);
  assert.equal(frame.hyperlinks[22],hyperlinks[0].target);

  const applyRow = replacement => accept(null,{...denseSnapshot,rows:[replacement,second]});
  assert.throws(() => applyRow({...row,cell_bytes:row.cell_bytes.slice(1)}),/dense Channel row/);
  assert.throws(() => applyRow({...row,cell_bytes:' '+row.cell_bytes.slice(1)}),/dense Channel row/);
  assert.throws(() => applyRow({...row,text:row.text+'a'}),/dense Channel row width/);
  assert.throws(() => applyRow({...row,attributes:[...row.attributes,...row.attributes]}),/dense Channel attributes/);
  assert.throws(() => applyRow({...row,runs:[]}),/dense Channel row/);
  const splitSurrogate = Buffer.from(bytes);
  splitSurrogate[8] = 1;
  assert.throws(() => applyRow({...row,cell_bytes:splitSurrogate.toString('base64')}),/dense Channel text/);
  const badContinuation = Buffer.from(bytes);
  badContinuation[16] = 1;
  assert.throws(() => applyRow({...row,cell_bytes:badContinuation.toString('base64')}),/wide span/);
  assert.throws(() => accept(null,{...denseSnapshot,stamp:{...stamp,version:CHANNEL_RENDITION_VERSION}}),/dense Channel row version/);
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

test('the wire byte limit counts an astral glyph across a scanning boundary as four bytes', () => {
  const prefix = '{"padding":"';
  const start = prefix + 'x'.repeat(8191-prefix.length) + '🙂';
  const end = '",' + JSON.stringify(snapshot).slice(1);
  const limit = 8*1024*1024;
  const encoded = start + 'x'.repeat(limit-Buffer.byteLength(start)-Buffer.byteLength(end)) + end;
  assert.equal(Buffer.byteLength(encoded),limit);
  assert.equal(acceptChannelFrame(null,encoded,owner).revision,snapshot.stamp.revision);
  assert.throws(() => acceptChannelFrame(null,encoded+' ',owner),/frame byte limit/);
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

const imageOwner = {...owner, frameVersion:5};
const imageId = '9007199254740993';
const image = {id:imageId,width:1,height:2,columns:1,rows:2,rgba:'/wAA/wD/AP8='};
const imageRow = (row, slice, column=1, id=imageId) => ({...row,image_slices:[{column,id,slice}]});
const imageSnapshot = {...snapshot,stamp:{...stamp,version:5},rows:[imageRow(first,0),imageRow(second,1)],images:[image]};
const imageDelta = {...delta,stamp:{...delta.stamp,version:5},rows:[{index:0,row:changed}]};
const acceptImage = (previous,value) => acceptChannelFrame(previous,JSON.stringify(value),imageOwner);

test('Channel row moves copy immutable base rows and retain images and links at their new positions', () => {
  const baseStamp = {...stamp,version:CHANNEL_FRAME_VERSION};
  const newest = {...second,cells:[{...cell,text:'Z'},cell]};
  const payload = {...snapshot,stamp:baseStamp,rows:[imageRow(first,0),second,newest],images:[image]};
  const base = accept(null,payload);
  const update = {kind:'delta',stamp:{...baseStamp,revision:delta.stamp.revision},base_revision:base.revision,
    rows:[],moves:[{from:1,to:0,count:2},{from:0,to:2,count:1}]};
  const moved = accept(base,update);
  const full = accept(null,{...payload,stamp:update.stamp,rows:[second,newest,imageRow(first,0)]});
  assert.deepEqual(moved,full);
  assert.equal(moved.rows[0],base.rows[1]);
  assert.equal(moved.rows[2],base.rows[0]);
  assert.equal(moved.rows[2].wrapped,true);
  assert.equal(moved.images[imageId],base.images[imageId]);
  assert.equal(moved.hyperlinks[22],hyperlinks[0].target);
  const cleared = accept(moved,{...update,stamp:{...update.stamp,revision:'9007199254740994'},
    base_revision:moved.revision,moves:[],rows:[{index:2,row:second}]});
  assert.deepEqual(Object.keys(cleared.images),[]);
  assert.deepEqual(Object.keys(cleared.hyperlinks),[]);
});

test('Channel row moves reject legacy or foreign bases, malformed spans, and conflicting row edits', () => {
  const payload = {...snapshot,stamp:{...stamp,version:CHANNEL_FRAME_VERSION}};
  const base = accept(null,payload);
  const move = {from:1,to:0,count:1};
  const update = {kind:'delta',stamp:{...payload.stamp,revision:delta.stamp.revision},base_revision:base.revision,
    rows:[],moves:[move]};
  for (const moves of [null, {}, [null], [{...move,count:0}], [{...move,count:2}], [{...move,from:0}],
    [{...move,to:-1}], [{...move,from:.5}], [move,move]]) {
    assert.throws(() => accept(base,{...update,moves}),/row move/);
  }
  assert.throws(() => accept(base,{...update,rows:[{index:0,row:second}]}),/row index/);
  assert.throws(() => accept(base,{...update,base_revision:'9007199254740991'}),/delta base/);
  assert.throws(() => accept(null,{...payload,moves:[move]}),/row move/);
  const legacy = acceptImage(null,imageSnapshot);
  assert.throws(() => acceptImage(legacy,{...imageDelta,moves:[move]}),/row move/);
  assert.throws(() => acceptChannelFrame(legacy,JSON.stringify(update),{...owner,frameVersion:CHANNEL_FRAME_VERSION}),/delta base/);
  assert.equal(base.rows[0][0].text,cluster.text);
});

test('Channel row moves still charge repeated base text against the entire frame limit', () => {
  const long = {wrapped:false,cells:Array(256).fill({...cell,text:'🪶'.repeat(16)})};
  const short = {wrapped:false,cells:Array(256).fill(cell)};
  const payload = {kind:'snapshot',stamp:{...stamp,version:CHANNEL_FRAME_VERSION},cols:256,
    rows:[...Array(31).fill(long),short,short]};
  const base = accept(null,payload);
  const update = {kind:'delta',stamp:{...payload.stamp,revision:delta.stamp.revision},base_revision:base.revision,
    rows:[],moves:[{from:0,to:31,count:2}]};
  assert.throws(() => accept(base,update),/text byte limit/);
  assert.equal(base.rows[31][0].text,' ');
});

test('Canvas copies pixels only from the painted base and redraws rows touched by either cursor', () => {
  const row = text => ({wrapped:false,cells:[{...cell,text},cell]});
  const payload = {kind:'snapshot',cols:2,rows:['A','B','C','D'].map(row),
    stamp:{...stamp,version:CHANNEL_FRAME_VERSION,cursor_row:1,cursor_visible:true}};
  const base = accept(null,payload);
  const next = accept(base,{kind:'delta',base_revision:base.revision,
    stamp:{...payload.stamp,revision:delta.stamp.revision,cursor_row:2},
    moves:[{from:1,to:0,count:3}],rows:[{index:3,row:row('Z')}]});
  const render = (before,scale) => {
    const copies = [], labels = [];
    const context = {setTransform(){},clearRect(){},save(){},beginPath(){},rect(){},clip(){},restore(){},
      fillRect(){},strokeRect(){},fillText(text,x,y){if (x === 0) labels.push([text,y]);},
      drawImage(source,...rect){assert.equal(source,canvas);copies.push(rect);}};
    const canvas={style:{},getContext:()=>context};
    paint(canvas,before,scale);
    labels.length = 0;
    paint(canvas,next,scale);
    return {copies,labels};
  };
  assert.deepEqual(render(base,1),{
    copies:[[0,24,24,72,0,0,24,72]],labels:[['B',2],['D',50],['Z',74]],
  });
  const skipped = accept(null,{...payload,stamp:{...payload.stamp,revision:'9007199254740991'}});
  for (const [before,scale] of [[skipped,1],[base,1.5]]) {
    assert.deepEqual(render(before,scale),{copies:[],labels:[['B',2],['C',26],['D',50],['Z',74]]});
  }
});

test('image slices reuse the exact base resource and release it with the last visible slice', () => {
  const full = acceptImage(null,imageSnapshot);
  const trimmed = acceptImage(full,imageDelta);
  assert.equal(trimmed.rows[0].imageSlices,undefined);
  assert.equal(trimmed.rows[1],full.rows[1]);
  assert.equal(trimmed.images[imageId],full.images[imageId]);
  assert.equal(trimmed.images[imageId].rgba,image.rgba);
  const moved = acceptImage(trimmed,{...imageDelta,base_revision:trimmed.revision,
    stamp:{...imageDelta.stamp,revision:'9007199254740994'},
    rows:[{index:0,row:imageRow(changed,1)},{index:1,row:second}],hyperlinks});
  assert.equal(moved.images[imageId],full.images[imageId]);
  const erased = acceptImage(moved,{...imageDelta,base_revision:moved.revision,
    stamp:{...imageDelta.stamp,revision:'9007199254740995'}});
  assert.deepEqual(Object.keys(erased.images),[]);
  assert.ok(full.images[imageId]);
  assert.throws(() => acceptImage(null,{...imageSnapshot,images:[]}),/unresolved/);
  assert.throws(() => acceptImage(full,{...imageDelta,images:[image]}),/identity/);
  assert.throws(() => acceptImage(full,{...imageSnapshot,stamp:imageDelta.stamp,
    images:[{...image,rgba:'AAAAAAAAAAA='}]}),/changed Channel image/);
});

test('resource generation, bounds, canonical RGBA and both resource byte budgets are enforced', () => {
  const check = (changes, error) => assert.throws(() => acceptImage(null,{...imageSnapshot,...changes}),error);
  check({stamp:{...imageSnapshot.stamp,resource_generation:'8'}},/foreign/);
  check({stamp:{...imageSnapshot.stamp,version:4}},/foreign/);
  check({stamp:{...imageSnapshot.stamp,screen:'alternate'}},/count or version/);
  check({images:[{...image,columns:2}]},/out-of-bounds/);
  check({images:[{...image,rgba:'/wAA/wD/AP9='}]},/base64/);
  check({images:[{...image,width:4096,height:4096}]},/image byte limit/);
  check({images:[{...image,id:'4'}]},/unresolved/);
  check({images:[image,{...image,id:'4'}]},/unreferenced/);
  check({rows:[imageRow(first,2),imageRow(second,1)]},/out-of-bounds/);
  assert.throws(() => accept(null,{...snapshot,images:[image]}),/version/);
  const large = {...image,width:512,height:1024,rows:1,rgba:'A'.repeat(2796203)+'='};
  const full = acceptImage(null,{...imageSnapshot,images:[large],rows:[first,imageRow(second,0)]});
  const small = {...image,id:'4'};
  assert.throws(() => acceptImage(full,{...imageDelta,images:[small],
    rows:[{index:0,row:imageRow(changed,0,1,small.id)}]}),/retained image limit/);
  assert.deepEqual(Object.keys(full.images),[imageId]);
});

const archiveOwner = {...owner,generation:3};
const archive = {
  version:5,owner:archiveOwner,revision:'9007199254741000',cols:2,offset:0,total_rows:2,
  text:'🐍\ne\u0301',
  rows:[
    {source:{row:'9007199254740993',column:0},paint:{wrapped:false,
      cells:[{...cell,text:'🐍',width:2},{...cell,width:0}],image_slices:[{column:0,id:imageId,slice:0}]},
      text_offsets:[0,2,2]},
    {source:{row:'9007199254740994',column:0},paint:{wrapped:false,cells:[{...cell,text:'e\u0301'}]},
      text_offsets:[3,5]},
  ],
  images:[{...image,columns:3}],
};

test('an archived page preserves wide glyphs, source identities and clipped rasters', () => {
  const projected = acceptHistoryProjection(JSON.stringify(archive),archiveOwner);
  assert.deepEqual(projected.offsets,[[0,2,2],[3,5]]);
  assert.equal(projected.text.slice(projected.offsets[1][0],projected.offsets[1][1]),'e\u0301');
  assert.equal(projected.rows[1].length,archive.cols);
  assert.equal(projected.images[imageId].columns,3);
  assert.equal(channelCounter(projected.sources[0].row),9007199254740993n);
  const withPrefix = {...archive,text:archive.text+'\nmore'};
  assert.throws(()=>acceptHistoryProjection(JSON.stringify(withPrefix),archiveOwner),/resources/);
  assert.equal(acceptHistoryProjection(JSON.stringify(withPrefix),archiveOwner,true).text,withPrefix.text);
});

test('an archived page rejects stale ownership, false cell offsets and conflicting source rows', () => {
  const admit = value => acceptHistoryProjection(JSON.stringify(value),archiveOwner);
  assert.throws(()=>admit({...archive,owner:{...archiveOwner,generation:2}}),/foreign/);
  assert.throws(()=>admit({...archive,text:'🐍\nxx'}),/copy range/);
  assert.throws(()=>admit({...archive,rows:[archive.rows[0],{...archive.rows[1],text_offsets:[2,5]}]}),/boundary/);
  assert.throws(()=>admit({...archive,rows:[archive.rows[0],{...archive.rows[1],source:archive.rows[0].source}]}),/row source/);
  assert.throws(()=>admit({...archive,images:[]}),/unresolved/);
});

test('Channel v4 and v5 optional cursor color accepts old frames and color-only deltas',()=>{
 for(const version of [4,5]){
  const full={...snapshot,stamp:{...stamp,version,cursor_visible:true,cursor_color:'#abcdef'}};
  const base=accept(null,full);
  assert.equal(base.cursor_color,'#abcdef');
  const next=accept(base,{kind:'delta',stamp:{...full.stamp,revision:'9007199254740993',cursor_color:'#123456'},
   base_revision:stamp.revision,rows:[],hyperlinks:[]});
  assert.equal(next.cursor_color,'#123456');
  assert.equal(next.rows[0],base.rows[0]);
  const legacy=accept(null,{...snapshot,stamp:{...stamp,version}});
  assert.equal(legacy.cursor_color,undefined);
  for(const cursor_color of [null,1,'blue','#abc','#11223344']){
   assert.throws(()=>accept(null,{...full,stamp:{...full.stamp,cursor_color}}),/cursor color/);
  }
 }
});

test('history padding uses the supplied default background and accepts legacy pages',()=>{
 const old=acceptHistoryProjection(JSON.stringify(archive),archiveOwner);
 assert.equal(old.rows[1].at(-1).background,'#111820');
 const next=acceptHistoryProjection(JSON.stringify({...archive,default_background:'#2468ac'}),archiveOwner);
 assert.equal(next.rows[1].at(-1).background,'#2468ac');
 assert.equal(next.rows[1][0].background,old.rows[1][0].background);
 for(const default_background of [null,1,'white','#abc','#12345678']){
  assert.throws(()=>acceptHistoryProjection(JSON.stringify({...archive,default_background}),archiveOwner),/default background/);
 }
});
