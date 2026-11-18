import test from 'node:test';import assert from 'node:assert/strict';
import {createFilePreview,localSnapshotPath} from '../web/file-preview.js';
const bytes=text=>new TextEncoder().encode(text).buffer;
function setup(){const shown=[],errors=[];return {shown,errors,...createFilePreview({render:x=>shown.push(x),onError:x=>errors.push(x)})};}
const file=(path,text)=>({webkitRelativePath:path,size:new TextEncoder().encode(text).length,arrayBuffer:async()=>bytes(text)});
test('only explicit root snapshots authorize bounded text reads',async()=>{
 const h=setup();await h.show('README.md');assert.equal(h.errors.length,1);
 assert.deepEqual(h.select([file('project/README.md','# Local'),file('project/src/App.tsx','<script>danger()</script>')]),['README.md','src/App.tsx']);
 await h.show('src/App.tsx');assert.deepEqual(h.shown.at(-1),{root:'project',path:'src/App.tsx',text:'<script>danger()</script>'});
 await h.show('secret.md');assert.equal(h.shown.at(-1),null);
});
test('root replacement is fail closed for traversal absolute remote ambiguous and disguised paths',()=>{
 for(const p of ['/etc/passwd','../x','a/../x','a/./x','file:///x','ssh://host/x','C:/x','a\\x','a//x','a/\u202ex','a/\n'])assert.equal(localSnapshotPath(p),null,p);
 for(const entries of [[file('../a.md','')],[file('a/x.md',''),file('b/y.md','')],[file('a/x.md',''),file('a/x.md','')],[file('bare.md','')]]){
  const h=setup();h.select([file('old/ok.md','ok')]);assert.deepEqual(h.select(entries),[]);assert.equal(h.errors.length,1);
 }
});
test('oversize and unsupported files are rejected before reads; UTF-8 and NUL checked',async()=>{
 for(const [path,size] of [['root/x.md',262145],['root/x.png',12]]){
  const h=setup();h.select([{webkitRelativePath:path,size,arrayBuffer(){assert.fail('unbounded read');}}]);await h.show(path.slice(5));assert.equal(h.errors.length,1);
 }
 for(const content of [new Uint8Array([0xff]).buffer,bytes('nul\0text')]){
  const h=setup();h.select([{webkitRelativePath:'root/x.md',size:content.byteLength,arrayBuffer:async()=>content}]);await h.show('x.md');assert.equal(h.shown.at(-1),null);assert.equal(h.errors.length,1);
 }
});
test('late read cannot resurrect closed or replaced preview',async()=>{
 for(const clear of [h=>h.dismiss(),h=>h.clear(),h=>h.select([file('new/x.md','new')])]){
  let resolve;const old=new Promise(r=>resolve=r),h=setup();h.select([{webkitRelativePath:'old/x.md',size:3,arrayBuffer:()=>old}]);const pending=h.show('x.md');clear(h);resolve(bytes('old'));await pending;assert.equal(h.shown.at(-1),null);
 }
});
test('only one pending read is admitted across repeated requests and root replacement',async()=>{
 let resolve,reads=0;const pending=new Promise(r=>resolve=r),h=setup();
 h.select([{webkitRelativePath:'root/old.md',size:3,arrayBuffer:()=>{reads++;return pending;}},file('root/new.md','new')]);
 const first=h.show('old.md');
 for(let i=0;i<100;i++)await h.show('old.md');assert.equal(reads,1);
 h.select([file('new/new.md','new')]);await h.show('new.md');assert.equal(h.shown.at(-1),null);
 resolve(bytes('old'));await first;assert.equal(h.shown.at(-1),null);
 await h.show('new.md');assert.equal(h.shown.at(-1).text,'new');
});

import {selectedPathAtCell} from '../web/file-preview.js';
const frame=text=>({cols:[...text].length,rows:[[...text].map(text=>({text,width:1}))]});
test('Ctrl-click candidate lookup matches selected exact tokens only',()=>{
 const f=frame('open `src/App.tsx` and README.md');
 assert.equal(selectedPathAtCell(f,0,8,['src/App.tsx']), 'src/App.tsx');
 assert.equal(selectedPathAtCell(f,0,23,['README.md']), 'README.md');
 for(const text of ['ssh://host/src/App.tsx','/src/App.tsx','prefixsrc/App.tsx','src/App.tsxmore'])assert.equal(selectedPathAtCell(frame(text),0,Math.min(8,text.length-1),['src/App.tsx']),null);
 assert.equal(selectedPathAtCell(frame('./src/App.tsx'),0,4,['src/App.tsx']), 'src/App.tsx');
 assert.equal(selectedPathAtCell(f,1,8,['src/App.tsx']),null);
});
import {installFilePreviewPanel} from '../web/file-preview-panel.js';
function panelHarness(){
 const elements=new Map();let epoch=1,current={...frame('src/App.tsx'),session:'pty-demo',epoch:1};
 const el=selector=>{if(!elements.has(selector))elements.set(selector,{value:'',textContent:'',hidden:false,handlers:{},addEventListener(n,fn){this.handlers[n]=fn;},replaceChildren(){this.children=[];},append(x){this.children.push(x);},focus(){this.focused=true;},getBoundingClientRect(){return {left:0,top:0,width:120,height:24};}});return elements.get(selector);};
 const doc={querySelector:el,createElement:()=>({})};
 const api=installFilePreviewPanel(doc,{getFrame:()=>current,getEpoch:()=>epoch});
 const click=(extra={})=>el('canvas').handlers.click({button:0,ctrlKey:true,currentTarget:el('canvas'),clientX:30,clientY:10,preventDefault(){},...extra});
 return {el,api,click,setEpoch:e=>epoch=e,setFrame:f=>current=f};
}
test('actual panel controller requires explicit local confirmation after Ctrl-left-click',async()=>{
 let reads=0;const h=panelHarness(),f=file('root/src/App.tsx','<script>not executed</script>');f.arrayBuffer=async()=>{reads++;return bytes('<script>not executed</script>');};
 h.el('#preview-folder').files=[f];h.el('#preview-folder').handlers.change();h.click();
 assert.equal(reads,0);assert.equal(h.el('#file-preview').hidden,false);assert.equal(h.el('#preview-title').textContent,'src/App.tsx');assert.equal(h.el('#preview-confirm-local').focused,true);
 await h.el('#preview-confirm-local').handlers.click();assert.equal(reads,1);assert.equal(h.el('#preview-source').textContent,'<script>not executed</script>');
 h.api.dismiss();assert.equal(h.el('#file-preview').hidden,true);assert.equal(h.el('#preview-source').textContent,'');
});
test('panel rejects wrong clicks and stale session confirmation without reading',async()=>{
 let reads=0;const h=panelHarness();h.el('#preview-folder').files=[{webkitRelativePath:'root/src/App.tsx',size:1,arrayBuffer:async()=>{reads++;return bytes('x');}}];h.el('#preview-folder').handlers.change();
 for(const extra of [{button:1},{ctrlKey:false},{altKey:true},{shiftKey:true},{metaKey:true},{defaultPrevented:true},{clientX:-1}]){h.click(extra);assert.equal(h.el('#file-preview').hidden,true);}
 h.click();h.setEpoch(2);await h.el('#preview-confirm-local').handlers.click();assert.equal(reads,0);assert.equal(h.el('#file-preview').hidden,true);
});
test('valid read keeps confirmation panel visible while loading and clear cancels late result',async()=>{
 let resolve;const waiting=new Promise(r=>resolve=r),h=panelHarness();
 h.el('#preview-folder').files=[{webkitRelativePath:'root/src/App.tsx',size:1,arrayBuffer:()=>waiting}];h.el('#preview-folder').handlers.change();h.click();
 const pending=h.el('#preview-confirm-local').handlers.click();assert.equal(h.el('#file-preview').hidden,false);assert.match(h.el('#preview-source').textContent,/Loading/);
 h.el('#preview-clear').handlers.click();resolve(bytes('x'));await pending;
 assert.equal(h.el('#file-preview').hidden,true);assert.equal(h.el('#preview-source').textContent,'');
});
test('repeated confirmation does not discard the admitted read or remain loading',async()=>{
 let resolve,reads=0;const waiting=new Promise(r=>resolve=r),h=setup();
 h.select([{webkitRelativePath:'root/x.md',size:1,arrayBuffer:()=>{reads++;return waiting;}}]);
 const first=h.show('x.md');await h.show('x.md');resolve(bytes('x'));await first;
 assert.equal(reads,1);assert.equal(h.shown.at(-1).text,'x');
});
test('large folder fails before enumeration and clears previous authority',async()=>{
 const h=setup();h.select([file('root/old.md','old')]);
 const huge={length:2049,[Symbol.iterator](){assert.fail('must not enumerate oversized selection');}};
 assert.deepEqual(h.select(huge),[]);assert.match(h.errors.at(-1),/Select one source file/);
 await h.show('old.md');assert.equal(h.shown.at(-1),null);
});
test('single-file snapshots avoid directory enumeration and retain bounded reads',async()=>{
 const h=setup();let reads=0;
 assert.deepEqual(h.selectFile({name:'App.tsx',size:3,arrayBuffer:async()=>{reads++;return bytes('src');}}),['App.tsx']);
 assert.equal(reads,0);await h.show('App.tsx');assert.equal(h.shown.at(-1).text,'src');
 for(const name of ['../x.md','src/x.md','/x.md','x\u202e.md']){
  assert.deepEqual(h.selectFile({name,size:1}),[]);await h.show('App.tsx');assert.equal(h.shown.at(-1),null);
 }
 h.selectFile({name:'big.md',size:262145,arrayBuffer(){assert.fail('oversize read');}});
 await h.show('big.md');assert.match(h.errors.at(-1),/256 KiB/);
});
test('actual panel preserves folder rejection and offers manual-only single file',async()=>{
 const h=panelHarness();
 h.el('#preview-folder').files={length:2049};h.el('#preview-folder').handlers.change();
 assert.match(h.el('#preview-notice').textContent,/2048-file/);
 let reads=0;
 h.el('#preview-single').files=[{name:'src.md',size:3,arrayBuffer:async()=>{reads++;return bytes('src');}}];
 h.el('#preview-single').handlers.change();
 h.setFrame({...frame('src.md'),session:'pty-demo',epoch:1});h.click();
 assert.equal(h.el('#file-preview').hidden,true);assert.equal(reads,0);
 h.el('#preview-files').value='src.md';await h.el('#preview-show').handlers.click();
 assert.equal(reads,1);assert.equal(h.el('#preview-source').textContent,'src');
 h.el('#preview-clear').handlers.click();assert.equal(h.el('#preview-single').value,'');
});
test('switching from pending directory read to single file invalidates late content',async()=>{
 let finish;const h=setup(),pending=new Promise(r=>finish=r);
 h.select([{webkitRelativePath:'root/old.md',size:3,arrayBuffer:()=>pending}]);
 const old=h.show('old.md');h.selectFile({name:'new.md',size:3,arrayBuffer:async()=>bytes('new')});
 finish(bytes('old'));await old;assert.equal(h.shown.at(-1),null);
 await h.show('new.md');assert.equal(h.shown.at(-1).text,'new');
});

test('location suffixes select only the explicitly authorized exact file',()=>{
 for(const token of ['src/App.tsx:12','./src/App.tsx:12:3','src/App.tsx:4294967295:4294967295']){
  const f=frame('see '+token+' next');
  for(let col=4;col<4+token.length;col++)assert.equal(selectedPathAtCell(f,0,col,['src/App.tsx']),'src/App.tsx',token+' cell '+col);
  assert.equal(selectedPathAtCell(f,0,4,[]),null);
 }
});
test('malformed and remote location references never partially resolve',()=>{
 for(const suffix of [':0',':01',':-1',':+1',':1:0',':1:02',':1:',':1:bad',':1:2:3',':4294967296',':1:4294967296',':99999999999999999999999',':12x',':12/path']){
  const token='src/App.tsx'+suffix;
  assert.equal(selectedPathAtCell(frame(token),0,5,['src/App.tsx']),null,token);
 }
 for(const token of ['/src/App.tsx:12','ssh://host/src/App.tsx:12','file:///src/App.tsx:12','prefixsrc/App.tsx:12'])
  assert.equal(selectedPathAtCell(frame(token),0,token.indexOf('App'),['src/App.tsx']),null,token);
});
test('panel location click still requires local confirmation and honors epoch',async()=>{
 const h=panelHarness();let reads=0;
 const f=file('root/src/App.tsx','source');f.arrayBuffer=async()=>{reads++;return bytes('source');};
 h.el('#preview-folder').files=[f];h.el('#preview-folder').handlers.change();
 h.setFrame({...frame('src/App.tsx:12:3'),session:'pty-demo',epoch:1});
 h.click({clientX:112});assert.equal(h.el('#preview-title').textContent,'src/App.tsx');assert.equal(reads,0);
 h.setEpoch(2);await h.el('#preview-confirm-local').handlers.click();assert.equal(reads,0);
 h.setEpoch(1);h.click({clientX:112});await h.el('#preview-confirm-local').handlers.click();assert.equal(reads,1);
});
test('sentence punctuation resolves complete authorized candidates conservatively',()=>{
 for(const token of ['README.md,','README.md.','README.md:12,','README.md:12:3。','README.md！？','README.md；','README.md，']){
  const f=frame('see '+token+' then');
  assert.equal(selectedPathAtCell(f,0,5,['README.md']),'README.md',token);
 }
 for(const text of ['「README.md」。','“README.md:12”，','《README.md》']){
  assert.equal(selectedPathAtCell(frame(text),0,2,['README.md']),'README.md',text);
 }
});
test('punctuation is not a license for partial token or URI matching',()=>{
 for(const text of ['README.md.js','README.md,backup','README.md，备份','README.md../secret','README.md:12,backup','https://host/README.md,','file:///README.md.','xREADME.md,']){
  assert.equal(selectedPathAtCell(frame(text),0,text.indexOf('README')+2,['README.md']),null,text);
 }
});
test('literal punctuation names remain exact and competing candidates fail closed',()=>{
 for(const name of ['README.md,','README.md.','README.md，']){
  const f=frame(name);
  assert.equal(selectedPathAtCell(f,0,2,[name]),name);
  assert.equal(selectedPathAtCell(f,0,2,['README.md',name]),null);
  assert.equal(selectedPathAtCell(f,0,name.length-1,['README.md',name]),null);
  assert.equal(selectedPathAtCell(f,0,name.length-1,['README.md']),null);
 }
 assert.equal(selectedPathAtCell(frame('a,b.md'),0,2,['a,b.md']),'a,b.md');
});

test('panel punctuation candidate requires confirmation and rejects ambiguous authorized names',async()=>{
 const h=panelHarness();let reads=0;
 const f=file('root/README.md','source');f.arrayBuffer=async()=>{reads++;return bytes('source');};
 h.el('#preview-folder').files=[f];h.el('#preview-folder').handlers.change();
 h.setFrame({...frame('README.md:12,'),session:'pty-demo',epoch:1});
 h.click({clientX:20});assert.equal(h.el('#preview-title').textContent,'README.md');assert.equal(reads,0);
 h.setEpoch(2);await h.el('#preview-confirm-local').handlers.click();assert.equal(reads,0);
 h.setEpoch(1);h.click({clientX:20});await h.el('#preview-confirm-local').handlers.click();assert.equal(reads,1);
 h.el('#preview-folder').files=[f,file('root/README.md,','literal')];h.el('#preview-folder').handlers.change();
 h.setFrame({...frame('README.md,'),session:'pty-demo',epoch:1});
 h.click({clientX:20});assert.equal(h.el('#file-preview').hidden,true);assert.equal(reads,1);
});

test('wide punctuation continuation does not become a prose filename hit',()=>{
 const f={cols:11,rows:[ [...'README.md'].map(text=>({text,width:1})).concat([{text:'。',width:2},{text:'',width:0}]) ]};
 assert.equal(selectedPathAtCell(f,0,2,['README.md']),'README.md');
 for(const col of [9,10]){
  assert.equal(selectedPathAtCell(f,0,col,['README.md']),null);
  assert.equal(selectedPathAtCell(f,0,col,['README.md','README.md。']),null);
 }
});
