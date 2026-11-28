import test from 'node:test';import assert from 'node:assert/strict';import fs from 'node:fs';
import {acceptFull,acceptUpdate,clearPaint,paint} from '../web/frames.js';
import {cursorTarget} from '../web/viewport.js';
const demo=()=>JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
test('accept actual Rust engine fixture and isolate external mutation',()=>{const raw=demo(),f=acceptFull(null,raw);raw.rows[0][0].text='X';assert.notEqual(f.rows[0][0].text,'X');assert.equal(f.cols,64);});

test('semantic deltas update only validated rows and require the exact base',()=>{
 const fixture=demo(),full=acceptUpdate(null,{kind:'snapshot',frame:fixture});
 const changed=fixture.rows[0];changed[0].text='X';
 const delta={kind:'delta',frame:{...fixture,base_revision:full.revision,revision:full.revision+1,
  rows:[{index:0,cells:changed}]}};
 const next=acceptUpdate(full,delta);
 assert.equal(next.rows[0][0].text,'X');assert.notEqual(full.rows[0][0].text,'X');
 assert.equal(next.rows[1],full.rows[1]);assert.notEqual(next.rows[0],full.rows[0]);
 changed[0].text='Y';assert.equal(next.rows[0][0].text,'X');
 for(const invalid of [
  {base_revision:0},{layout:full.layout+1},{epoch:full.epoch+1},{session:'foreign'},
  {rows:[...delta.frame.rows,...delta.frame.rows]}, {rows:[{index:128,cells:changed}]},
  {rows:Array(full.rows.length+1).fill(null)}, {cursor_col:full.cols},
 ]){
  assert.throws(()=>acceptUpdate(full,{kind:'delta',frame:{...delta.frame,...invalid}}));
  assert.equal(full.revision,fixture.revision);
 }
 assert.throws(()=>acceptUpdate(null,delta));
 const cursorOnly=acceptUpdate(full,{kind:'delta',frame:{...delta.frame,rows:[],cursor_col:2}});
 assert.equal(cursorOnly.cursor_col,2);assert.equal(cursorOnly.rows[0],full.rows[0]);
});
test('reject foreign session/epoch, stale revisions and invalid sizes atomically',()=>{const old=acceptFull(null,demo());for(const change of [{session:'other'},{epoch:2},{revision:0},{revision:1},{revision:2,cols:0},{revision:2,cursor_col:999}]){assert.throws(()=>acceptFull(old,{...demo(),...change}));assert.equal(old.revision,1);}});
test('reject malformed cells and non-advanced layout resize',()=>{for(const text of ['\x1b','xx','']){const f=demo();f.rows[0][0].text=text;assert.throws(()=>acceptFull(null,f));}const f=demo();f.revision=2;f.rows.pop();assert.throws(()=>acceptFull(acceptFull(null,demo()),f));});
test('paint uses cell-local canvas operations, no terminal interpretation',()=>{
 const {canvas,glyphs}=paintCalls(acceptFull(null,demo()),2);
 assert.equal(glyphs.length,144);
 assert.equal(canvas.width,1536);
});

test('Rust-owned hidden cursor is not painted',()=>{let cursors=0;const g={setTransform(){},clearRect(){},scale(){},fillRect(){},save(){},beginPath(){},rect(){},clip(){},fillText(){},restore(){},strokeRect(){cursors++}};const canvas={style:{},getContext:()=>g};paint(canvas,acceptFull(null,{...demo(),cursor_visible:false}));assert.equal(cursors,0);assert.throws(()=>acceptFull(null,{...demo(),cursor_visible:0}));});

test('reject control and unpaired scalar cells; allow astral Unicode',()=>{for(const text of ['\u0085','\ud800','\udfff']){const f=demo();f.rows[0][0].text=text;assert.throws(()=>acceptFull(null,f));}const f=demo();f.rows[0][0].text='😀';assert.equal(acceptFull(null,f).rows[0][0].text,'😀');});
test('canvas scale is bounded even with hostile or invalid device scale',()=>{const g={setTransform(x){assert.equal(x,1)},clearRect(){},scale(x){assert.equal(x,1)},fillRect(){},save(){},beginPath(){},rect(){},clip(){},fillText(){},restore(){},strokeRect(){}};for(const scale of [Infinity,NaN,-1,1000]){const canvas={style:{},getContext:()=>g};paint(canvas,acceptFull(null,demo()),scale);assert.equal(canvas.width,768);}});
test('largest permitted grid stays under 16 megapixel backing allocation',()=>{const g={setTransform(){},clearRect(){},scale(){},fillRect(){},save(){},beginPath(){},rect(){},clip(){},fillText(){},restore(){},strokeRect(){}};const raw=demo();raw.cols=256;raw.rows=Array.from({length:128},()=>Array.from({length:256},()=>({text:' ',foreground:'#ffffff',background:'#000000',width:1,style:0})));const canvas={style:{},getContext:()=>g};paint(canvas,acceptFull(null,raw),4);assert.ok(canvas.width*canvas.height<=16777216);});
test('actual Rust Unicode frame keeps two-column paint and styled colors',()=>{
 const raw=JSON.parse(fs.readFileSync(new URL('../web/unicode-frame.json',import.meta.url),'utf8'));
 const f=acceptFull(null,raw,{session:'unicode-demo',epoch:1});
 assert.equal(f.rows[0][0].text,'中');assert.equal(f.rows[0][0].width,2);assert.equal(f.rows[0][1].width,0);
 assert.equal(f.rows[0][0].foreground,'#123456');assert.equal(f.rows[0][0].background,'#ff0000');assert.equal(f.rows[0][0].style,15);
 assert.equal(f.rows[1][0].foreground,'#2472c8');assert.equal(f.rows[1][0].background,'#cd3131');
 const calls=[];const g={setTransform(){},clearRect(){},scale(){},fillRect(...args){calls.push(['background-or-decoration',...args]);},save(){},beginPath(){},rect(...args){calls.push(['clip',...args]);},clip(){},fillText(text){calls.push(['glyph',text,this.font,this.globalAlpha]);},restore(){},strokeRect(){}};
 paint({style:{},getContext:()=>g},f);
 const firstGlyph=calls.findIndex(c=>c[0]==='glyph');
 const backgroundCoverage=calls.slice(0,firstGlyph).filter(c=>c[0]==='background-or-decoration')
  .reduce((cells,[,,,w,h])=>{assert.equal(h,24);return cells+w/12;},0);
 assert.equal(backgroundCoverage,4*24);
 assert.deepEqual(calls.find(c=>c[0]==='clip'),['clip',0,0,24,24]);
 assert.deepEqual(calls.find(c=>c[0]==='glyph'),['glyph','中','italic bold 18px monospace',1]);
 assert.equal(calls.filter(c=>c[0]==='glyph'&&c[1]==='中').length,1);
 assert.ok(calls.some(c=>c[0]==='glyph'&&c[1]==='D'&&c[3]===0.5));
});
test('reject inconsistent wide-pair geometry atomically',()=>{for(const mutate of [f=>f.rows[0][0].width=0,f=>f.rows[0][0].width=2,f=>f.rows[0][63].width=2,f=>f.rows[0][0].style=32]){const f=demo();mutate(f);assert.throws(()=>acceptFull(null,f));}});
test('Rust cursor shapes paint at exact cell geometry and invalid shapes reject',()=>{const raw=demo();raw.cursor_row=2;raw.cursor_col=3;for(const shape of ['block','beam','underline']){const fills=[],strokes=[];const g={setTransform(){},clearRect(){},scale(){},fillRect(...x){fills.push(x)},save(){},beginPath(){},rect(){},clip(){},fillText(){},restore(){},strokeRect(...x){strokes.push(x)}};paint({style:{},getContext:()=>g},acceptFull(null,{...raw,cursor_shape:shape}));if(shape==='block')assert.deepEqual(strokes.at(-1),[36.5,48.5,11,23]);if(shape==='beam')assert.deepEqual(fills.at(-1),[36,48,2,24]);if(shape==='underline')assert.deepEqual(fills.at(-1),[36,70,12,2]);}assert.throws(()=>acceptFull(null,{...raw,cursor_shape:'invalid'}));assert.throws(()=>acceptFull(null,{...raw,pending_wrap:1}));});


test('oversized cell text is rejected before Unicode iteration and leaves old frame intact',()=>{
 const old=acceptFull(null,demo());
 const raw=demo();raw.revision=2;raw.rows[0][0].text='x'.repeat(1_000_000);
 const original=String.prototype[Symbol.iterator];let visited=0;
 String.prototype[Symbol.iterator]=function*(){visited+=this.length;yield* original.call(this);};
 try {assert.throws(()=>acceptFull(old,raw),/cell/);} finally {String.prototype[Symbol.iterator]=original;}
 assert.equal(visited,0,'invalid text must be rejected without a scalar array');
 assert.equal(old.revision,1);
 assert.equal(acceptFull(old,{...demo(),revision:2}).revision,2);
});

// Model the destructive Canvas dimension setter separately from a physical clear.
function observedCanvas() {
 const writes=[], css=[], draws=[];
 let w=300,h=150,scale=1,level=0;
 const g={
  setTransform(a,b,c,d,e,f){assert.deepEqual([b,c,d,e,f],[0,0,a,0,0]);scale=a;},
  scale(x){scale*=x;},
  clearRect(x,y,width,height){assert.equal(level,0);assert.deepEqual([x,y,width,height,scale],[0,0,w,h,1]);draws.length=0;draws.push(['clear',width,height]);},
  fillRect(...args){draws.push(['fill',scale,this.fillStyle,...args]);},
  fillText(text){draws.push(['text',scale,this.fillStyle,text]);},
  save(){level++;},restore(){level--;},beginPath(){},rect(){},clip(){assert.equal(level,1);},
  strokeRect(...args){draws.push(['cursor',scale,...args]);},
 };
 const style=new Proxy({}, {set(target,key,value){css.push([key,value]);target[key]=value;return true;}});
 const canvas={style,getContext(){return g;},offsetLeft:4,offsetTop:6,
  get width(){return w;},set width(x){w=x;scale=1;level=0;draws.length=0;writes.push(['width',x]);},
  get height(){return h;},set height(x){h=x;scale=1;level=0;draws.length=0;writes.push(['height',x]);}};
 return {canvas,g,writes,css,draws,get clipDepth(){return level;}};
}
function smallPaintFrame({cols=2,rows=2,cursor_visible=true,background='#112233',text='X'}={}) {
 const cell={text,width:1,style:0,foreground:'#ffffff',background};
 return acceptFull(null,{...demo(),cols,rows:Array.from({length:rows},()=>Array(cols).fill(cell)),
  cursor_row:0,cursor_col:0,cursor_visible});
}
test('same backing size avoids dimension writes but repaints colors and clears stale cursor and overlays',()=>{
 const {canvas,writes,css,draws}=observedCanvas();
 paint(canvas,smallPaintFrame(),1);
 assert.deepEqual(writes,[['width',24],['height',48]]);
 assert.deepEqual(css,[['width','24px'],['height','48px']]);
 assert.ok(draws.some(([kind])=>kind==='cursor'));
 draws.push(['stale-overlay']);
 // Fractional DPR changes the transform, but rounds to the same pixel size.
 paint(canvas,smallPaintFrame({cursor_visible:false,background:'#445566',text:'Y'}),1.01);
 assert.deepEqual(writes,[['width',24],['height',48]]);
 assert.deepEqual(css,[['width','24px'],['height','48px']]);
 assert.deepEqual(draws[0],['clear',24,48]);
 assert.deepEqual(draws.slice(1,5).map(op=>op.slice(0,3)),Array(4).fill(['fill',1.01,'#445566']));
 assert.equal(draws.filter(op=>op[0]==='text'&&op[1]===1.01&&op[3]==='Y').length,4);
 assert.ok(!draws.some(([kind])=>kind==='cursor'||kind==='stale-overlay'));
 paint(canvas,smallPaintFrame(),1.01);
 assert.equal(draws.find(op=>op[0]==='text')[1],1.01,'transform must not accumulate across frames');
});

test('DPR and grid changes resize only affected dimensions and preserve CSS-space viewport targets',()=>{
 const {canvas,writes,css,draws}=observedCanvas();
 const initial=smallPaintFrame();
 const viewport={clientWidth:4,clientHeight:6,scrollWidth:50,scrollHeight:65};
 const position=cursorTarget(initial,canvas,viewport);
 paint(canvas,initial,1);
 paint(canvas,initial,2);
 assert.deepEqual(writes,[['width',24],['height',48],['width',48],['height',96]]);
 assert.deepEqual(css,[['width','24px'],['height','48px']]);
 assert.deepEqual(cursorTarget(initial,canvas,viewport),position);
 paint(canvas,smallPaintFrame({cols:3}),2);
 assert.deepEqual(writes.at(-1),['width',72]);assert.equal(writes.length,5);
 assert.deepEqual(css.at(-1),['width','36px']);assert.equal(css.length,3);
 assert.deepEqual(draws[0],['clear',72,96]);
 assert.equal(draws.filter(op=>op[0]==='text').length,6);
});

test('failed paint unwinds the cell clip before a same-geometry full repaint',()=>{
 const h=observedCanvas(), frame=smallPaintFrame();
 const fill=h.g.fillText;
 h.g.fillText=()=>{throw new Error('transient paint failure');};
 assert.throws(()=>paint(h.canvas,frame),/transient paint failure/);
 assert.equal(h.clipDepth,0);
 h.g.fillText=fill;
 paint(h.canvas,frame);
 assert.equal(h.writes.length,2);assert.equal(h.clipDepth,0);
 assert.equal(h.draws.filter(op=>op[0]==='text').length,4);
});

test('dense ASCII reuses opaque glyphs by color and style with direct drawing for other cells',()=>{
 const {canvas,g,draws}=observedCanvas();
 const atlasBackgrounds=[],atlasGlyphs=[],copies=[];
 const texture={
  globalAlpha:1,
  setTransform(){},save(){},beginPath(){},rect(){},clip(){},restore(){this.globalAlpha=1;},
  fillRect(...area){atlasBackgrounds.push([this.fillStyle,...area]);},
  fillText(...text){atlasGlyphs.push([this.font,this.globalAlpha,this.fillStyle,...text]);},
 };
 const source={getContext:()=>texture};
 canvas.ownerDocument={createElement(tag){assert.equal(tag,'canvas');return source;}};
 g.globalAlpha=1;
 g.globalCompositeOperation='source-over';
 g.drawImage=(...copy)=>copies.push(copy);
 const raw=smallPaintFrame({cols:40,rows:16,cursor_visible:false});
 const original=raw.rows[0][0];
 const palette=[original,{...original,background:'#667788'},
  {...original,style:17},{...original,foreground:'#ad4170'}];
 const rows=raw.rows.map(row=>row.map((_,x)=>palette[x%palette.length]));
 rows[0][5]={...original,text:'界',width:2};
 rows[0][6]={...original,text:' ',width:0};
 rows[0][8]={...original,text:'Y',style:4};
 const frame=acceptFull(null,{...raw,rows});
 paint(canvas,frame,1);
 assert.deepEqual(atlasBackgrounds,[
  ['#112233',0,0,12,24],['#667788',12,0,12,24],
  ['#112233',24,0,12,24],['#112233',36,0,12,24],
 ]);
 assert.deepEqual(atlasGlyphs,[
  ['18px monospace',1,'#ffffff','X',0,2],['18px monospace',1,'#ffffff','X',12,2],
  ['bold 18px monospace',0.5,'#ffffff','X',24,2],['18px monospace',1,'#ad4170','X',36,2],
 ]);
 assert.equal(copies.length,637);
 assert.deepEqual(copies.slice(0,4),palette.map((_,i)=>[source,i*12,0,12,24,i*12,0,12,24]));
 assert.deepEqual(draws.filter(([kind])=>kind==='text').map(draw=>draw[3]),['界','Y']);

 copies.length=0;
 const changed=rows[0].map((cell,x)=>x===20?{...cell,text:'Z'}:cell);
 g.clearRect=()=>{draws.length=0;};
 const next=deltaFrom(frame,{rows:[{index:0,cells:changed}]});
 paint(canvas,next,1);
 assert.deepEqual(draws.filter(([kind])=>kind==='text').map(draw=>draw[3]),['Z']);
 paint(canvas,next,1.25);
 assert.equal(copies.length,0);
 assert.equal(atlasGlyphs.length,4);
});

test('a failed truecolor pixel probe keeps drawing through Canvas2D without repeating calibration',()=>{
 const {canvas,g,draws}=observedCanvas();
 let probes=0;
 const texture={
  setTransform(){},save(){},beginPath(){},rect(){},clip(){},restore(){},
  fillRect(){},fillText(){},
  getImageData(x,y,width,height){
   probes++;
   return {data:new Uint8ClampedArray(width*height*4)};
  },
 };
 canvas.ownerDocument={createElement:()=>({getContext:()=>texture})};
 g.globalAlpha=1;
 g.globalCompositeOperation='source-over';
 g.direction='ltr';
 g.textAlign='start';
 g.createImageData=()=>assert.fail('an incompatible raster allocated a band');
 g.putImageData=()=>assert.fail('an incompatible raster wrote pixels');
 g.drawImage=()=>assert.fail('these color pairs do not repeat');
 const raw=smallPaintFrame({cols:80,rows:24,cursor_visible:false});
 const blank=acceptFull(null,{...raw,rows:raw.rows.map(row=>row.map(cell=>({...cell,text:' '})))});
 paint(canvas,blank,1);
 assert.equal(probes,0);

 const rows=raw.rows.map((row,y)=>row.map((cell,x)=>({
  ...cell,background:'#'+(y*raw.cols+x).toString(16).padStart(6,'0'),
 })));
 for (let revision=2;revision<=3;revision++) {
  paint(canvas,acceptFull(null,{...raw,rows,revision,layout:revision}),1);
  assert.equal(draws.filter(([kind])=>kind==='text').length,raw.cols*rows.length);
  assert.equal(probes,1);
 }
});

test('truecolor bands overwrite opaque backgrounds and blend partial glyph pixels exactly',()=>{
 const {canvas,g}=observedCanvas();
 const rects=[], ink=[];
 const channels=color=>color.slice(1).match(/../g).map(part=>parseInt(part,16));
 const blend=(fg,bg,alpha)=>Math.floor(fg*(alpha+1)/256)+Math.floor(bg*(256-alpha)/256);
 const texture={
  setTransform(){},save(){},beginPath(){},rect(){},clip(){},restore(){},
  fillRect(x,y,w,h){rects.push([x,y,w,h,channels(this.fillStyle)]);},
  fillText(_text,x,y){ink.push([x,y-2,channels(this.fillStyle)]);},
  getImageData(_x,_y,width,height){
   const data=new Uint8ClampedArray(width*height*4);
   for(const [x,y,w,h,color] of rects) {
    for(let dy=0;dy<h;dy++)for(let dx=0;dx<w;dx++)data.set([...color,255],((y+dy)*width+x+dx)*4);
   }
   for(const [x,y,color] of ink) for(const [dx,alpha] of [[0,255],[1,67]]) {
    const at=(y*width+x+dx)*4;
    if(data[at+3]) {
     for(let i=0;i<3;i++)data[at+i]=blend(color[i],data[at+i],alpha);
    } else data.set([...color,alpha],at);
   }
   return {data};
  },
 };
 canvas.ownerDocument={createElement:()=>({getContext:()=>texture})};
 Object.assign(g,{globalAlpha:1,globalCompositeOperation:'source-over',direction:'ltr',textAlign:'start'});
 g.clearRect=()=>assert.fail('successful opaque raster writes already cover the damage');
 g.createImageData=(width,height)=>({data:new Uint8ClampedArray(width*height*4),width,height});
 let firstPixels;
 g.putImageData=(image,x,y,left,top,width,height)=>{
  if(y===0) {
   assert.deepEqual([x,left,top,width],[0,0,0,canvas.width]);
   assert.ok(height>=24);
   firstPixels=[...image.data.subarray(0,12)];
  }
 };
 const raw=smallPaintFrame({cols:80,rows:24,cursor_visible:false});
 const rows=raw.rows.map((row,y)=>row.map((cell,x)=>({
  ...cell,foreground:'#1f7bd3',background:'#'+(0x234567+y*raw.cols+x).toString(16),
 })));
 paint(canvas,acceptFull(null,{...raw,rows}),1);
 const fg=[31,123,211], bg=[35,69,103];
 assert.deepEqual(firstPixels,[...fg,255,...fg.map((value,i)=>blend(value,bg[i],67)),255,...bg,255]);
});

function paintCalls(frame,scale) {
 const backgrounds=[],glyphs=[],clears=[];
 let drawingText=false,transform=1;
 const g={
  setTransform(dpr){transform=dpr;},clearRect(...rect){drawingText=false;clears.push([transform,...rect]);},
  fillRect(x,y,w,h){if(!drawingText)backgrounds.push([this.fillStyle,transform,x,y,w,h]);},
  save(){drawingText=true;},restore(){},beginPath(){},rect(){},clip(){},strokeRect(){},
  fillText(text,x,y){glyphs.push([text,x,y]);},
 };
 const canvas={style:{},getContext:()=>g};
 paint(canvas,frame,scale);
 return {backgrounds,glyphs,clears,canvas};
}

test('opaque background spans match the per-cell physical pixel model at integer DPR',()=>{
 const raw=demo(), cols=9, rows=3;
 const colors=['#000000','#112233','#aa4411'];
 const palettes=[[0,0,1,1,1,2,2,0,0],[2,2,1,0,0,2,0,0,0],[1,1,1,1,1,1,1,1,1]];
 const cells=palettes.map(row=>row.map(index=>({text:'·',width:1,style:0,foreground:'#010203',background:colors[index]})));
 cells[0][1]={...cells[0][1],text:'界',width:2};
 cells[0][2]={...cells[0][2],text:' ',width:0}; // Separate background is preserved under a wide glyph.
 const frame=acceptFull(null,{...raw,cols,rows:cells,cursor_row:0,cursor_col:1,cursor_visible:false});
 for(const dpr of [1,2,3,4]){
  const {backgrounds,clears,glyphs,canvas}=paintCalls(frame,dpr);
  const tileWidth=12*dpr,tileHeight=24*dpr;
  assert.deepEqual(clears,[[1,0,0,cols*tileWidth,rows*tileHeight]]);
  assert.equal(backgrounds.length,10);
  assert.equal(glyphs.length,cols*rows-1);
  assert.deepEqual(glyphs.filter(([text])=>text==='界'),[['界',12,2]]);
  const buffer=new Uint8Array(canvas.width*canvas.height);
  for(const [color,scale,x,y,w,h] of backgrounds){
   assert.equal(scale,dpr);assert.equal(h,24,'never merge vertically');
   assert.equal(x%12,0);assert.equal(w%12,0);assert.ok(x+w<=cols*12);
   for(let yy=y*dpr;yy<(y+h)*dpr;yy++){
    for(let xx=x*dpr;xx<(x+w)*dpr;xx++){
     const index=yy*canvas.width+xx;
     assert.equal(buffer[index],0,'each physical pixel receives exactly one background');
     buffer[index]=colors.indexOf(color)+1;
    }
   }
  }
  for(let y=0;y<canvas.height;y++){
   for(let x=0;x<canvas.width;x++){
    assert.equal(buffer[y*canvas.width+x],1+palettes[Math.floor(y/tileHeight)][Math.floor(x/tileWidth)]);
   }
  }
 }
});

test('Rust demo and PTY frames reduce fillRect calls from cells to rows without skipping glyphs',()=>{
 for(const [file,oldCalls,spans] of [['demo',512,8],['pty-smoke',960,12]]){
  const raw=JSON.parse(fs.readFileSync(new URL(`../web/${file}-frame.json`,import.meta.url),'utf8'));
  const frame=acceptFull(null,raw,{session:raw.session,epoch:raw.epoch});
  const {backgrounds,glyphs,clears}=paintCalls(frame,1);
  assert.equal(frame.cols*frame.rows.length,oldCalls);
  assert.equal(backgrounds.length,spans);
  const visibleGlyphs=frame.rows.flat().filter(c=>c.width>0&&c.text!==' ').length;
  assert.equal(glyphs.length,visibleGlyphs);
  assert.deepEqual(clears,[[1,0,0,frame.cols*12,frame.rows.length*24]]);
  assert.ok(backgrounds.every(([,,x,,w,h])=>x===0&&w===frame.cols*12&&h===24));
 }
});

test('fractional and allocation-clamped DPR preserve cell-by-cell background rasterization',()=>{
 const fractional=paintCalls(smallPaintFrame({cols:8,rows:2}),1.25);
 assert.equal(fractional.backgrounds.length,16);
 assert.ok(fractional.backgrounds.every(([_,dpr,__,___,w,h])=>dpr===1.25&&w===12&&h===24));
 const large=paintCalls(smallPaintFrame({cols:256,rows:128}),4);
 assert.equal(large.backgrounds.length,32768);
 const ratio=Math.sqrt(16777216/(256*12*128*24));
 assert.ok(large.backgrounds.every(([_,dpr,__,___,w,h])=>dpr===ratio&&w===12&&h===24));
 assert.ok(large.canvas.width*large.canvas.height<=16777216);
});

function deltaFrom(base, fields={}) {
 return acceptUpdate(base,{kind:'delta',frame:{...base,base_revision:base.revision,
  revision:base.revision+1,rows:[],...fields}});
}

test('validated row deltas repaint changed cells and the old and new cursor footprints',()=>{
 const {canvas,g,writes}=observedCanvas();
 const initial=smallPaintFrame({cols:2,rows:4});
 paint(canvas,initial,1.25);
 const clears=[],texts=[],cursors=[];
 g.clearRect=(...area)=>clears.push(area);
 g.fillText=(_,x,y)=>texts.push([x,y]);
 g.strokeRect=(...area)=>cursors.push(area);

 const changed=initial.rows[2].map(c=>({...c,text:'Y'}));
 const next=deltaFrom(initial,{rows:[{index:2,cells:changed}]});
 paint(canvas,next,1.25);
 assert.deepEqual(clears.splice(0),[[0,60,30,30]]);
 assert.deepEqual(texts.splice(0),[[0,50],[12,50]]);
 assert.equal(cursors.length,0);

 const moved=deltaFrom(next,{cursor_row:3});
 paint(canvas,moved,1.25);
 assert.deepEqual(clears.splice(0),[[0,0,15,30],[0,90,15,30]]);
 assert.deepEqual(texts.splice(0),[[0,2],[0,74]]);
 assert.deepEqual(cursors.splice(0),[[.5,72.5,11,23]]);

 const hidden=deltaFrom(moved,{cursor_visible:false});
 paint(canvas,hidden,1.25);
 assert.deepEqual(clears.splice(0),[[0,90,15,30]]);
 assert.deepEqual(texts.splice(0),[[0,74]]);
 paint(canvas,deltaFrom(hidden,{pending_wrap:true,cursor_row:1}),1.25);
 assert.deepEqual([clears,texts,cursors],[[],[],[]]);
 assert.deepEqual(writes,[['width',30],['height',120]]);
});

test('distant edits and cursor moves preserve large gaps and repaint complete wide cells',()=>{
 const demoFrame=smallPaintFrame({cols:28,rows:1});
 const row=demoFrame.rows[0].slice();
 row[6]={...row[6],text:'界',width:2};
 row[7]={...row[7],text:' ',width:0};
 const initial=acceptFull(null,{...demoFrame,rows:[row],cursor_col:22});
 const {canvas,g}=observedCanvas();
 paint(canvas,initial,1.25);
 const clears=[],glyphs=[],cursors=[];
 g.clearRect=(...area)=>clears.push(area);
 g.fillText=(text,x)=>glyphs.push([text,x]);
 g.strokeRect=(...area)=>cursors.push(area);

 const changed=row.slice();
 changed[2]={...row[2],text:'Y'};
 changed[7]={...row[7],background:'#445566'};
 const next=deltaFrom(initial,{cursor_col:26,rows:[{index:0,cells:changed}]});
 paint(canvas,next,1.25);
 assert.deepEqual(clears.splice(0),[[30,0,90,30],[330,0,75,30]]);
 assert.deepEqual(glyphs.splice(0),[
  ['Y',24],['X',36],['X',48],['X',60],['界',72],
  ['X',264],['X',276],['X',288],['X',300],['X',312],
 ]);
 assert.deepEqual(cursors.splice(0),[[312.5,.5,11,23]]);

 paint(canvas,deltaFrom(next,{cursor_col:7}),1.25);
 assert.deepEqual(clears,[[90,0,30,30],[390,0,15,30]]);
 assert.deepEqual(glyphs,[['界',72],['X',312]]);
 assert.deepEqual(cursors,[[84.5,.5,11,23]]);
});

test('horizontal cell damage requires aligned backing pixels',()=>{
 const initial=smallPaintFrame({cols:4,rows:2});
 const changed=initial.rows[0].map((c,x)=>x===2?{...c,text:'Y'}:c);
 const next=deltaFrom(initial,{rows:[{index:0,cells:changed}]});
 for (const [scale,area,draws,cursor] of [
  [1.25,[30,0,15,30],['Y'],0],
  [1.125,[0,0,54,27],['X','X','Y','X'],1],
 ]) {
  const {canvas,g}=observedCanvas();
  paint(canvas,initial,scale);
  const clears=[],texts=[],cursors=[];
  g.clearRect=(...rect)=>clears.push(rect);
  g.fillText=text=>texts.push(text);
  g.strokeRect=(...rect)=>cursors.push(rect);
  paint(canvas,next,scale);
  assert.deepEqual(clears,[area]);
  assert.deepEqual(texts,draws);
  assert.equal(cursors.length,cursor);
 }
});

test('reset, fractional row edges and interrupted deltas fall back to a full repaint',()=>{
 const {canvas,g,writes}=observedCanvas();
 const initial=smallPaintFrame({cols:2,rows:4});
 const next=deltaFrom(initial,{cursor_row:2});
 paint(canvas,initial,1);
 const clears=[];
 g.clearRect=(...area)=>clears.push(area);
 const fill=g.fillText;
 g.fillText=()=>{throw new Error('interrupted');};
 assert.throws(()=>paint(canvas,next,1),/interrupted/);
 assert.deepEqual(clears.splice(0),[[0,0,12,24],[0,48,12,24]]);
 g.fillText=fill;
 paint(canvas,next,1);
 assert.deepEqual(clears.splice(0),[[0,0,24,96]]);

 clearPaint(canvas);
 paint(canvas,next,1);
 assert.deepEqual(clears.splice(0),[[0,0,24,96]]);
 paint(canvas,next,1.01);
 paint(canvas,deltaFrom(next,{cursor_row:3}),1.01);
 assert.deepEqual(clears,[[0,0,24,96],[0,0,24,96]]);
 assert.deepEqual(writes,[['width',24],['height',96],['width',24]]);
});

test('optional cursor color validates and repaints an otherwise empty delta',()=>{
 const raw={...demo(),cursor_visible:true,cursor_shape:'block'};
 const base=acceptFull(null,raw);
 assert.equal(base.cursor_color,undefined);
 for(const color of ['#123456','#AbCdEf']){
  assert.equal(acceptFull(null,{...raw,cursor_color:color}).cursor_color,color);
 }
 for(const cursor_color of [null,1,'red','#123','#11223344','url(x)']){
  assert.throws(()=>acceptFull(null,{...raw,cursor_color}),/cursor color/);
 }
 const colors=[];
 const g={setTransform(){},clearRect(){},scale(){},fillRect(){},save(){},beginPath(){},rect(){},clip(){},fillText(){},restore(){},strokeRect(){colors.push(this.strokeStyle);}};
 const canvas={style:{},getContext:()=>g};
 paint(canvas,base,1);
 assert.equal(colors.at(-1),'#9ac6ff');
 const next=acceptUpdate(base,{kind:'delta',frame:{...raw,base_revision:base.revision,
  revision:base.revision+1,rows:[],cursor_color:'#123456'}});
 assert.equal(next.rows[0],base.rows[0]);
 paint(canvas,next,1);
 assert.equal(colors.at(-1),'#123456');
 const reset=acceptUpdate(next,{kind:'delta',frame:{...raw,base_revision:next.revision,
  revision:next.revision+1,rows:[]}});
 paint(canvas,reset,1);
 assert.equal(colors.at(-1),'#9ac6ff');
});
