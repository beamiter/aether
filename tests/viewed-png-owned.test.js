import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,mkdir,readFile,rm,stat,writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {deflateSync,inflateSync} from 'node:zlib';
import {installFilePreviewPanel} from '../web/file-preview-panel.js';

function crc32(bytes){
 let crc=0xffffffff;
 for(const byte of bytes){crc^=byte;for(let i=0;i<8;i++)crc=(crc>>>1)^((crc&1)?0xedb88320:0);}
 return (crc^0xffffffff)>>>0;
}
function chunk(name,data=Buffer.alloc(0)){
 const content=Buffer.concat([Buffer.from(name),data]), out=Buffer.alloc(data.length+12);
 out.writeUInt32BE(data.length);content.copy(out,4);out.writeUInt32BE(crc32(content),out.length-4);return out;
}
function png(rgb){
 const header=Buffer.alloc(13);header.writeUInt32BE(1);header.writeUInt32BE(1,4);header[8]=8;header[9]=2;
 return Buffer.concat([Buffer.from('89504e470d0a1a0a','hex'),chunk('IHDR',header),
  chunk('IDAT',deflateSync(Buffer.from([0,...rgb]))),chunk('IEND')]);
}
// Test decoder validates fixture bytes but does not stand in for native decode QA.
async function decodeFixture(blob){
 const png=Buffer.from(await blob.arrayBuffer()), idat=png.readUInt32BE(33);
 assert.equal(png.toString('ascii',37,41),'IDAT');
 assert.equal(png.readUInt32BE(41+idat),crc32(png.subarray(37,41+idat)));
 const pixels=inflateSync(png.subarray(41,41+idat),{maxOutputLength:4});
 assert.equal(pixels.length,4);assert.equal(pixels[0],0);
 return [...pixels.subarray(1)];
}
function harness(draws){
 let epoch=1, outerFrame;
 const elements=new Map(), doc={querySelector:el,createElement:()=>({})};
 function el(id){
  if(!elements.has(id))elements.set(id,{value:'',handlers:{},addEventListener(event,fn){this.handlers[event]=fn;},
   replaceChildren(){this.children=[];},append(child){this.children.push(child);},focus(){doc.activeElement=this;},
   getBoundingClientRect(){return {left:0,top:0,width:outerFrame.cols,height:1};},
   getContext(){return {drawImage(bitmap){draws.push(bitmap.rgb);}};}});
  return elements.get(id);
 }
 const panel=installFilePreviewPanel(doc,{getFrame:()=>outerFrame,getEpoch:()=>epoch});
 function click(text,offset){
  outerFrame={session:'pty-demo',epoch,cols:text.length,rows:[[...text].map(text=>({text,width:1}))]};
  let prevented=false;el('canvas').handlers.click({button:0,ctrlKey:true,currentTarget:el('canvas'),clientX:offset+.5,clientY:.5,
   preventDefault(){prevented=true;}});
  return prevented;
 }
 return {el,click,panel,nextEpoch(){epoch++;panel.dismiss();}};
}
test('owned same-name PNGs: exact path confirmation, sibling isolation and stale decode',async()=>{
 const root=await mkdtemp(join(tmpdir(),'aether-owned-viewed-'));
 const savedDecoder=globalThis.createImageBitmap, reads=[], draws=[], closes=[];
 let signal, release;
 const decodeEntered=new Promise(resolve=>{signal=resolve;});
 const decodeBlocked=new Promise(resolve=>{release=resolve;});
 try{
  const files=[];
  for(const [folder,rgb] of [['left',[255,0,0]],['right',[0,0,255]]]){
   await mkdir(join(root,folder));const path=join(root,folder,'image.png');
   await writeFile(path,png(rgb));
   files.push({webkitRelativePath:'owned/'+folder+'/image.png',size:(await stat(path)).size,
    async arrayBuffer(){reads.push(folder);return Uint8Array.from(await readFile(path)).buffer;}});
  }
  let waitForBlue=true;
  globalThis.createImageBitmap=async blob=>{
   const rgb=await decodeFixture(blob);
   if(rgb[2]&&waitForBlue){signal();await decodeBlocked;}
   return {width:1,height:1,rgb,close(){closes.push(rgb);}};
  };
  const h=harness(draws);
  h.el('#preview-folder').files=files;h.el('#preview-folder').handlers.change();
  assert.equal(h.click('Viewed image.png',10),false);
  assert.equal(h.click('Viewed file:///tmp/left/image.png',23),false);
  assert.deepEqual(reads,[]);
  assert.equal(h.click('Viewed left/image.png',12),true);
  assert.deepEqual(reads,[]);
  await h.el('#preview-confirm-local').handlers.click();
  await h.el('#preview-confirm-local').handlers.click();
  assert.deepEqual(reads,['left']);assert.deepEqual(draws,[[255,0,0]]);

  assert.equal(h.click('Viewed right/image.png',13),true);
  const stale=h.el('#preview-confirm-local').handlers.click();
  // Bound on a failure path where admission never reaches the decoder.
  await Promise.race([decodeEntered,stale.then(()=>assert.fail('no pending image decode'))]);
  h.nextEpoch();release();await stale;
  assert.equal(h.el('#file-preview').hidden,true);
  assert.deepEqual(draws,[[255,0,0]]);
  assert.deepEqual(closes,[[255,0,0],[0,0,255]]);

  waitForBlue=false;
  assert.equal(h.click('Viewed right/image.png',13),true);
  await h.el('#preview-confirm-local').handlers.click();
  assert.deepEqual(reads,['left','right','right']);
  assert.deepEqual(draws,[[255,0,0],[0,0,255]]);
  assert.deepEqual(closes,[[255,0,0],[0,0,255],[0,0,255]]);
 }finally{
  release();globalThis.createImageBitmap=savedDecoder;
  await rm(root,{recursive:true,force:true});
 }
});
