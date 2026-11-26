import test from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {createFilePreview} from '../web/file-preview.js';
import {installFilePreviewPanel} from '../web/file-preview-panel.js';
import {previewDom} from './support/preview-dom.js';

test('owned disk text fixtures verify byte caps and visible panel errors',async()=>{
 const dir=await fs.mkdtemp(path.join(os.tmpdir(),'aether-cap-owned-'));
 const reads=new Map(),shown=[],errors=[];
 try{
  const wrap=async(name,size,declared=size)=>{
   const target=path.join(dir,name);await fs.writeFile(target,Buffer.alloc(size,65));
   assert.equal((await fs.stat(target)).size,size);
   reads.set(name,0);
   return {name,webkitRelativePath:'owned/'+name,size:declared,arrayBuffer:async()=>{
    reads.set(name,reads.get(name)+1);return Uint8Array.from(await fs.readFile(target)).buffer;
   }};
  };
  const small=await wrap('100.md',100*1024),large=await wrap('300.md',300*1024);
  const exact=await wrap('256.md',256*1024),overflow=await wrap('256plus.md',256*1024+1);
  const changed=await wrap('changed.md',300*1024,100*1024);
  const loader=createFilePreview({render:x=>shown.push(x),onError:x=>errors.push(x)});
  loader.select([small,large,exact,overflow,changed]);
  await loader.show('100.md');assert.equal(shown.at(-1).text.length,102400);assert.equal(reads.get('100.md'),1);
  await loader.show('300.md');assert.equal(shown.at(-1),null);assert.equal(reads.get('300.md'),0);
  assert.equal(errors.at(-1),'Preview exceeds the 256 KiB text limit.');
  await loader.show('256.md');assert.equal(shown.at(-1).text.length,262144);
  await loader.show('256plus.md');assert.equal(reads.get('256plus.md'),0);assert.equal(shown.at(-1),null);
  await loader.show('changed.md');assert.equal(reads.get('changed.md'),1);assert.equal(shown.at(-1),null);
  assert.equal(errors.at(-1),'Preview exceeds the 256 KiB text limit.');
  // Real installer, model DOM: this is not a WebKit/window acceptance test.
  const {doc,el}=previewDom();
  installFilePreviewPanel(doc);
  el('#preview-folder').files=[small,large];el('#preview-folder').handlers.change();
  el('#preview-files').value='100.md';await el('#preview-show').handlers.click();
  assert.equal(el('#preview-source').textContent.length,102400);
  el('#preview-files').value='300.md';await el('#preview-show').handlers.click();
  assert.equal(el('#preview-notice').textContent,'Preview exceeds the 256 KiB text limit.');
  assert.equal(el('#file-preview').hidden,true);assert.equal(el('#preview-source').textContent,'');
  assert.equal(reads.get('300.md'),0);
  // A refusal must not poison the next explicit bounded preview.
  el('#preview-files').value='100.md';await el('#preview-show').handlers.click();
  assert.equal(el('#file-preview').hidden,false);assert.equal(el('#preview-notice').textContent,'');
  assert.equal(el('#preview-source').textContent.length,102400);
 }finally{await fs.rm(dir,{recursive:true,force:true});}
});
