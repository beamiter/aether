import test from 'node:test';import assert from 'node:assert/strict';
import {rasterInfo,MAX_IMAGE_BYTES} from '../web/raster-preview.js';
import {createFilePreview} from '../web/file-preview.js';
function png(w=1,h=1,extra=[]){
 const chunk=(name,data=[])=>{const b=new Uint8Array(data.length+12);new DataView(b.buffer).setUint32(0,data.length);b.set([...name].map(c=>c.charCodeAt()),4);b.set(data,8);return [...b];};
 const header=new Uint8Array(13),v=new DataView(header.buffer);v.setUint32(0,w);v.setUint32(4,h);header[8]=8;header[9]=6;
 return new Uint8Array([137,80,78,71,13,10,26,10,...chunk('IHDR',header),...extra.flatMap(n=>chunk(n)),...chunk('IDAT',[1]),...chunk('IEND')]).buffer;
}
function jpeg(w=1,h=1){
 return new Uint8Array([255,216,255,192,0,11,8,h>>8,h&255,w>>8,w&255,1,1,17,0,255,218,0,8,1,1,0,0,63,0,1,255,217]).buffer;
}
test('raster admission bounds dimensions and rejects active/animated/unsupported containers',()=>{
 assert.deepEqual(rasterInfo(png(),'x.png'),{width:1,height:1,mime:'image/png'});
 assert.deepEqual(rasterInfo(jpeg(),'x.jpg'),{width:1,height:1,mime:'image/jpeg'});
 for(const b of [png(0),png(4097),png(4096,4096),png(1,1,['acTL']),png(1,1,['IHDR']),png().slice(0,-1),new Uint8Array(MAX_IMAGE_BYTES+1).buffer])assert.throws(()=>rasterInfo(b,'x.png'));
 for(const [b,p] of [[png(),'x.jpg'],[jpeg(),'x.png'],[new TextEncoder().encode('<svg/>').buffer,'x.png'],[jpeg().slice(0,-1),'x.jpg']])assert.throws(()=>rasterInfo(b,p));
 const progressive=new Uint8Array(jpeg());progressive[3]=194;assert.throws(()=>rasterInfo(progressive.buffer,'x.jpg'));
 const trailing=new Uint8Array(jpeg().byteLength+1);trailing.set(new Uint8Array(jpeg()));assert.throws(()=>rasterInfo(trailing.buffer,'x.jpg'));
});
function harness(decodeImage){
 const shown=[],errors=[];return {shown,errors,...createFilePreview({render:x=>shown.push(x),onError:x=>errors.push(x),decodeImage})};
}
const file=b=>({name:'x.png',webkitRelativePath:'root/x.png',size:b.byteLength,arrayBuffer:async()=>b});
test('decoder runs only after admission; bitmap is consumed and closed',async()=>{
 let calls=0,closed=0;const h=harness(async()=>{calls++;return {width:1,height:1,close(){closed++;}};});
 assert.deepEqual(h.select([file(png())]),['x.png']);assert.equal(calls,0);await h.show('x.png');assert.equal(calls,1);assert.equal(closed,1);assert.equal(h.shown.at(-1).mime,'image/png');
 h.select([file(png(9000))]);await h.show('x.png');assert.equal(calls,1);
 h.select([{...file(png()),size:MAX_IMAGE_BYTES+1,arrayBuffer(){assert.fail('oversize read');}}]);await h.show('x.png');assert.equal(calls,1);
});
test('one decoder in flight; clear or replacement invalidates and closes late bitmap',async()=>{
 for(const invalidate of [h=>h.dismiss(),h=>h.clear(),h=>h.selectFile(file(png()))]){
  let resolve,calls=0,closed=0;const pending=new Promise(r=>resolve=r);
  const h=harness(()=>{calls++;return pending;});h.select([file(png())]);const read=h.show('x.png');await Promise.resolve();
  invalidate(h);for(let i=0;i<20;i++)await h.show('x.png');assert.equal(calls,1);
  resolve({width:1,height:1,close(){closed++;}});await read;assert.equal(closed,1);assert.equal(h.shown.at(-1),null);
 }
});
test('mismatching decoder output is closed, rejected and not displayed',async()=>{
 let closed=0;const h=harness(async()=>({width:4097,height:1,close(){closed++;}}));h.select([file(png())]);await h.show('x.png');assert.equal(closed,1);assert.equal(h.shown.at(-1),null);assert.match(h.errors.at(-1),/dimensions/);
});
import {installFilePreviewPanel} from '../web/file-preview-panel.js';
test('panel image confirmation draws only admitted bitmap, clears canvas and rejects stale epoch',async()=>{
 const elements=new Map();let epoch=1,reads=0,closed=0,draws=0;
 const el=s=>{if(!elements.has(s))elements.set(s,{value:'',handlers:{},addEventListener(n,f){this.handlers[n]=f;},replaceChildren(){this.children=[];},append(x){this.children.push(x);},focus(){},getBoundingClientRect(){return {left:0,top:0,width:50,height:10};},getContext(){return {drawImage(){draws++;}};}});return elements.get(s);};
 const old=globalThis.createImageBitmap;globalThis.createImageBitmap=async()=>({width:1,height:1,close(){closed++;}});
 try{
  const h=installFilePreviewPanel({querySelector:el,createElement:()=>({})},{getEpoch:()=>epoch,getSession:()=>'pty-demo',getFrame:()=>({session:'pty-demo',epoch,cols:5,rows:[ [...'x.png'].map(text=>({text,width:1})) ]})});
  const f=file(png());f.arrayBuffer=async()=>{reads++;return png();};
  el('#preview-folder').files=[f];el('#preview-folder').handlers.change();
  const click=()=>el('canvas').handlers.click({button:0,ctrlKey:true,currentTarget:el('canvas'),clientX:20,clientY:2,preventDefault(){}});
  click();assert.equal(reads,0);epoch=2;await el('#preview-confirm-local').handlers.click();assert.equal(reads,0);
  click();await el('#preview-confirm-local').handlers.click();assert.equal(reads,1);assert.equal(draws,1);assert.equal(closed,1);assert.equal(el('#preview-raster').hidden,false);
  h.dismiss();assert.equal(el('#preview-raster').hidden,true);assert.equal(el('#preview-raster').width,1);
 }finally{globalThis.createImageBitmap=old;}
});
test('compressed PNG metadata and multi-picture JPEG APP2 never reach decode',()=>{
 for(const chunk of ['iCCP','zTXt','iTXt'])assert.throws(()=>rasterInfo(png(1,1,[chunk]),'x.png'));
 const original=new Uint8Array(jpeg()),app2=new Uint8Array(original.length+4);app2.set(original.subarray(0,2));app2.set([255,226,0,2],2);app2.set(original.subarray(2),6);
 assert.throws(()=>rasterInfo(app2.buffer,'x.jpg'));
});
import {readFileSync} from 'node:fs';
test('valid static PNG and baseline JPEG fixtures pass admission',()=>{
 const fixtures=JSON.parse(readFileSync(new URL('./raster-fixtures.json',import.meta.url),'utf8'));
 for(const [ext,base64] of Object.entries(fixtures)){
  const b=Uint8Array.from(Buffer.from(base64,'base64')).buffer;
  assert.deepEqual(rasterInfo(b,'fixture.'+ext),{width:2,height:3,mime:ext==='png'?'image/png':'image/jpeg'});
 }
});
test('metadata admission failures do not call native decoder',async()=>{
 let calls=0;const h=harness(async()=>{calls++;assert.fail('metadata reached decoder');});
 for(const name of ['iCCP','zTXt','iTXt','eXIf','junk']){
  h.select([file(png(1,1,[name]))]);await h.show('x.png');assert.equal(calls,0);
 }
});
test('JPEG baseline sampling and scan component tables are bounded',()=>{
 for(const [offset,value] of [[13,0],[13,255],[14,4],[21,4],[20,2]]){
  const b=new Uint8Array(jpeg());b[offset]=value;assert.throws(()=>rasterInfo(b.buffer,'x.jpg'),String(offset));
 }
});
