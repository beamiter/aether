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
test('common source names and extensions are admitted only by explicit snapshot selection', async () => {
 const h = setup();
 const names = ['README', 'LICENSE', 'Makefile', 'Dockerfile', 'Cargo.lock',
  '.gitignore', '.editorconfig', 'src/guide.MDX', 'src/style.scss', 'src/app.mjs',
  'src/tool.cjs', 'src/options.jsonc', 'src/types.hpp', 'src/main.go', 'src/Main.java', 'README:12'];
 h.select(names.map(name => file('root/' + name, 'local')));
 for (const name of names) {
  await h.show(name);
  assert.equal(h.shown.at(-1).path, name);
  assert.equal(h.shown.at(-1).text, 'local');
 }
 assert.equal(selectedPathAtCell(frame('README:12'), 0, 3, ['README', 'README:12']), 'README:12');
 h.select([file('root/secret.lock', 'excluded')]);
 await h.show('secret.lock');
 assert.equal(h.shown.at(-1), null);
 assert.match(h.errors.at(-1), /Unsupported preview type/);
});
test('oversize and unsupported files are rejected before reads; UTF-8 and NUL checked',async()=>{
 for(const [path,size] of [['root/x.md',262145],['root/x.gif',12]]){
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
import {previewDom} from './support/preview-dom.js';
function panelHarness(){
 let epoch=1,current={...frame('src/App.tsx'),session:'pty-demo',epoch:1};
 const {doc,el}=previewDom();
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
test('alternate screen does not propose a local file preview', () => {
 const h = panelHarness();
 h.el('#preview-folder').files = [file('root/src/App.tsx', 'local')];
 h.el('#preview-folder').handlers.change();
 const reference = {...frame('src/App.tsx'), session:'pty-demo', epoch:1};
 h.setFrame({...reference, screen:'alternate'});
 h.click({preventDefault() { assert.fail('alternate screen owns the click'); }});
 assert.equal(h.el('#file-preview').hidden, true);
 h.setFrame({...reference, screen:'primary'});
 h.click();
 assert.equal(h.el('#file-preview').hidden, false);
});
test('Shift and Ctrl request an inert local preview while the primary screen tracks application mouse', async () => {
 const h=panelHarness();
 let reads=0;
 const source=file('root/src/App.tsx','selected local source');
 source.arrayBuffer=async()=>{reads++;return bytes('selected local source');};
 h.el('#preview-folder').files=[source];
 h.el('#preview-folder').handlers.change();
 h.setFrame({...frame('src/App.tsx'),session:'pty-demo',epoch:1,screen:'primary',
  mouse:{tracking:'button',generation:2}});
 h.click();
 assert.equal(h.el('#file-preview').hidden,true);
 h.click({shiftKey:true});
 assert.equal(h.el('#file-preview').hidden,false);
 assert.equal(reads,0);
 await h.el('#preview-confirm-local').handlers.click();
 assert.equal(reads,1);
});
test('only complete references may touch frame row boundaries', () => {
 const wrapped = {
  cols:9,
  rows:[frame('foo/bar/ ').rows[0], frame('README.md').rows[0], frame('  next.md').rows[0]],
  row_wrapped:[true,true,false],
 };
 assert.equal(selectedPathAtCell(wrapped, 1, 2, ['README.md']), null);
 assert.equal(selectedPathAtCell(wrapped, 2, 4, ['next.md']), 'next.md');
 for (const row_wrapped of [[true,false,false], [false,true,false]]) {
  assert.equal(selectedPathAtCell({...wrapped,row_wrapped}, 1, 2, ['README.md']), null);
 }
 assert.equal(selectedPathAtCell({...wrapped,row_wrapped:[false,false,false]}, 1, 2, ['README.md']), 'README.md');
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
test('repeated stale terminal confirmations never become manual file authorization',async()=>{
 const h=panelHarness();let reads=0;
 const f=file('root/src/App.tsx','source');f.arrayBuffer=async()=>{reads++;return bytes('source');};
 h.el('#preview-folder').files=[f];h.el('#preview-folder').handlers.change();h.click();h.setEpoch(2);
 for(let i=0;i<3;i++){await h.el('#preview-confirm-local').handlers.click();await h.el('#preview-show').handlers.click();}
 assert.equal(reads,0);
 // Only an explicit manual selection change clears terminal provenance.
 h.el('#preview-files').handlers.change();await h.el('#preview-show').handlers.click();
 assert.equal(reads,1);
});
test('authorized literal numeric suffix wins over location without read fallback',async()=>{
 assert.equal(localSnapshotPath('root/a.md:12:3'),'root/a.md:12:3');
 for(const p of ['C:/a.md','ssh://host/a.md','root:a/a.md','root/a.md:evil'])assert.equal(localSnapshotPath(p),null);
 for(const token of ['a.md:12','./a.md:12','a.md:12,']){
  assert.equal(selectedPathAtCell(frame(token),0,2,['a.md','a.md:12']),'a.md:12');
  assert.equal(selectedPathAtCell(frame(token),0,2,['a.md']),'a.md');
 }
 const h=setup();let fallback=0;
 h.select([file('root/a.md:12','literal'),{...file('root/a.md','base'),arrayBuffer:async()=>{fallback++;return bytes('base');}}]);
 await h.show('a.md:12');assert.equal(h.shown.at(-1).text,'literal');assert.equal(fallback,0);
 h.select([{...file('root/a.md:12',''),arrayBuffer:async()=>{throw new Error('NotReadableError');}},{...file('root/a.md','base'),arrayBuffer:async()=>{fallback++;return bytes('base');}}]);
 await h.show('a.md:12');assert.equal(h.shown.at(-1),null);assert.equal(fallback,0);assert.match(h.errors.at(-1),/NotReadable/);
});
test('real Linux numeric-colon files remain distinct and missing literal never reads sibling',async()=>{
 const fs=await import('node:fs/promises'),os=await import('node:os'),path=await import('node:path');
 const dir=await fs.mkdtemp(path.join(os.tmpdir(),'aether-literal-'));
 try{
  await fs.writeFile(path.join(dir,'a.md'),'base');await fs.writeFile(path.join(dir,'a.md:12'),'literal');
  const wrap=async name=>({webkitRelativePath:'root/'+name,size:(await fs.stat(path.join(dir,name))).size,arrayBuffer:async()=>{const b=await fs.readFile(path.join(dir,name));return Uint8Array.from(b).buffer;}});
  const h=setup();h.select([await wrap('a.md'),await wrap('a.md:12')]);
  await h.show('a.md:12');assert.equal(h.shown.at(-1).text,'literal');
  await fs.unlink(path.join(dir,'a.md:12'));await h.show('a.md:12');
  assert.equal(h.shown.at(-1),null);assert.match(h.errors.at(-1),/ENOENT/);
 }finally{await fs.rm(dir,{recursive:true,force:true});}
});
test('panel confirms literal filename, preserves epoch and never reads location sibling',async()=>{
 const h=panelHarness();let literal=0,base=0;
 const a=file('root/a.md:12','literal'),b=file('root/a.md','base');
 a.arrayBuffer=async()=>{literal++;return bytes('literal');};b.arrayBuffer=async()=>{base++;return bytes('base');};
 h.el('#preview-folder').files=[a,b];h.el('#preview-folder').handlers.change();
 h.setFrame({...frame('a.md:12'),session:'pty-demo',epoch:1});h.click({clientX:20});
 assert.equal(h.el('#preview-title').textContent,'a.md:12');assert.equal(literal,0);assert.equal(base,0);
 h.setEpoch(2);await h.el('#preview-confirm-local').handlers.click();assert.equal(literal,0);
 h.setEpoch(1);h.click({clientX:20});await h.el('#preview-confirm-local').handlers.click();
 assert.equal(literal,1);assert.equal(base,0);
});

test('drive-relative prefixes are rejected before numeric filename suffix parsing',()=>{
 for(const value of ['C:12','c:0','D:12:3','Z:/file.md','C:12/file.md'])assert.equal(localSnapshotPath(value),null,value);
 assert.equal(localSnapshotPath('root/a.md:12'),'root/a.md:12');
 assert.equal(localSnapshotPath('root/C:12'),'root/C:12');
});
test('preview Escape dismisses only plain unhandled non-IME keys and restores safe focus',()=>{
 const h=panelHarness();h.el('#preview-folder').files=[file('root/src/App.tsx','text')];h.el('#preview-folder').handlers.change();h.click();
 const panel=h.el('#file-preview');
 assert.equal(typeof panel.handlers.keydown,'function');
 for(const extra of [{isComposing:true},{keyCode:229},{ctrlKey:true},{altKey:true},{shiftKey:true},{metaKey:true},{defaultPrevented:true}]){
  panel.handlers.keydown({key:'Escape',...extra,preventDefault(){assert.fail('must not consume');},stopPropagation(){assert.fail('must not consume');}});
  assert.equal(panel.hidden,false);
 }
 h.el('canvas').focus();
 panel.handlers.keydown({key:'Escape',preventDefault(){assert.fail('terminal-focused Escape must remain terminal-owned');},stopPropagation(){assert.fail('outside focus');}});
 assert.equal(panel.hidden,false);
 h.el('#preview-confirm-local').focus();
 let stopped=false;
 panel.handlers.keydown({key:'Escape',preventDefault(){},stopPropagation(){stopped=true;}});
 assert.equal(panel.hidden,true);assert.equal(stopped,true);assert.equal(h.el('#preview-show').focused,true);assert.equal(h.el('#preview-notice').textContent,'Preview closed. No terminal input sent.');
 panel.handlers.keydown({key:'Escape',repeat:true,preventDefault(){assert.fail('hidden repeat must remain on preview button');},stopPropagation(){assert.fail('hidden repeat');}});
 h.click();h.el('#preview-close').handlers.click();assert.equal(h.el('canvas').focused,true);
 h.click();h.setEpoch(2);h.el('#preview-close').handlers.click();
 assert.equal(h.el('#preview-show').focused,true);
});
test('Escape invalidates pending preview and late text cannot reopen sidebar',async()=>{
 const h=panelHarness();let finish;const pending=new Promise(r=>finish=r);
 const f=file('root/src/App.tsx','text');f.arrayBuffer=()=>pending;
 h.el('#preview-folder').files=[f];h.el('#preview-folder').handlers.change();h.click();
 const reading=h.el('#preview-confirm-local').handlers.click();
 h.el('#file-preview').handlers.keydown({key:'Escape',preventDefault(){},stopPropagation(){}});
 finish(bytes('text'));await reading;
 assert.equal(h.el('#file-preview').hidden,true);assert.equal(h.el('#preview-source').textContent,'');
});

test('confirmed source reference jumps within the selected file and exposes reported column', async () => {
 const h = panelHarness();
 h.el('#preview-folder').files = [file('root/src/App.tsx', 'one\r\ntwo\r\nthree\rfour')];
 await h.el('#preview-folder').handlers.change();
 h.setFrame({...frame('src/App.tsx:3:7'), session: 'pty-demo', epoch: 1});
 h.click({clientX: 110});
 assert.equal(h.el('#preview-details').textContent, 'Requested line 3 · reported column 7');
 await h.el('#preview-confirm-local').handlers.click();
 assert.equal(h.el('#preview-source').textContent, 'one\ntwo\nthree\\u{d}four');
 assert.equal(h.el('#preview-line-status').textContent, 'Line 3 of 3 · reported column 7');
 assert.equal(h.el('#preview-source').style['--preview-line-offset'], 2);
 assert.ok(h.el('#preview-content').scrollTop > 0);
 assert.equal(h.el('#preview-confirmation').hidden, true);

 h.setFrame({...frame('src/App.tsx:30'), session: 'pty-demo', epoch: 1});
 h.click();
 await h.el('#preview-confirm-local').handlers.click();
 assert.match(h.el('#preview-line-status').textContent, /Line 30 is outside/);
 assert.equal(h.el('#preview-copy-line').disabled, true);
 assert.equal(h.el('#preview-source').dataset.highlight, undefined);
});

test('literal occurrence navigation wraps within a line and across lines', async () => {
 const copied = [];
 const {doc, el} = previewDom({writeText(value) { copied.push(value); }});
 installFilePreviewPanel(doc);
 el('#preview-single').files = [{...file('', 'first [word] then [WORD] [word]\n雪 [Word]'), name:'source.md'}];
 await el('#preview-single').handlers.change();
 el('#preview-files').value = 'source.md';
 await el('#preview-show').handlers.click();

 const query = el('#preview-query'), source = el('#preview-source');
 query.value = '[word]';
 query.handlers.input();
 assert.equal(el('#preview-matches').textContent, '2 matches');
 assert.deepEqual(source.children.filter(node => node.tag === 'mark').map(mark => mark.textContent), ['[word]', '[word]']);
 el('#preview-next').handlers.click();
 assert.equal(el('#preview-matches').textContent, 'Match 1 of 2');
 el('#preview-next').handlers.click();
 assert.equal(el('#preview-matches').textContent, 'Match 2 of 2');
 el('#preview-next').handlers.click();
 assert.equal(el('#preview-matches').textContent, 'Match 1 of 2');

 el('#preview-match-case').checked = false;
 el('#preview-match-case').handlers.change();
 assert.equal(el('#preview-matches').textContent, '4 matches');
 el('#preview-prev').handlers.click();
 assert.equal(el('#preview-matches').textContent, 'Match 3 of 4');
 el('#preview-next').handlers.click();
 assert.equal(el('#preview-line-status').textContent, 'Line 2 of 2');
 assert.equal(source.children.find(mark => mark.classList?.contains('active')).textContent, '[Word]');
 await el('#preview-copy-line').handlers.click();
 await el('#preview-copy-text').handlers.click();
 await el('#preview-copy-path').handlers.click();
 assert.deepEqual(copied, ['雪 [Word]', source.textContent, 'source.md']);
});

test('find retains at most 4096 matches and a capped source remains navigable', async () => {
 const h = panelHarness();
 h.el('#preview-folder').files = [file('root/src/App.tsx', 'x'.repeat(4100) + '\n'.repeat(10000) + 'tail')];
 await h.el('#preview-folder').handlers.change();
 h.click();
 await h.el('#preview-confirm-local').handlers.click();
 const query = h.el('#preview-query');
 query.value = 'x';
 query.handlers.input();
 assert.equal(h.el('#preview-matches').textContent, '4096+ matches');
 assert.equal(h.el('#preview-source').children.filter(node => node.tag === 'mark').length, 4096);
 h.el('#preview-line').value = '10001';
 h.el('#preview-line-form').handlers.submit({preventDefault() {}});
 assert.equal(h.el('#preview-line-status').textContent, 'Line 10001 of 10001');
 assert.ok(h.el('#preview-content').scrollTop > 200000);
 await h.el('#preview-copy-line').handlers.click();
 assert.equal(h.el('#preview-copy-fallback').hidden, false);
 assert.equal(h.el('#preview-copy-value').value, 'tail');
 assert.equal(h.el('#preview-copy-value').selected, true);
});

test('reload reauthorizes the visible target and preserves navigation through success and failure', async () => {
 const h = panelHarness();
 const picker = h.el('#preview-folder'), reload = h.el('#preview-reload');
 const source = h.el('#preview-source'), query = h.el('#preview-query');
 picker.files = [file('root/src/App.tsx', 'hello hello\nold')];
 await picker.handlers.change();
 h.click();
 await h.el('#preview-confirm-local').handlers.click();
 query.value = 'hello';
 query.handlers.input();
 h.el('#preview-next').handlers.click();
 h.el('#preview-next').handlers.click();
 h.el('#preview-content').scrollLeft = 100;

 reload.handlers.click();
 assert.equal(picker.clicked, true);
 let finish;
 picker.files = [{...file('root/src/App.tsx', 'hello hello\nnew'), arrayBuffer: () => new Promise(resolve => { finish = resolve; })}];
 const pending = picker.handlers.change();
 assert.equal(source.textContent, 'hello hello\nold');
 assert.equal(reload.disabled, true);
 finish(bytes('hello hello\nnew'));
 await pending;
 assert.equal(source.textContent, 'hello hello\nnew');
 assert.equal(query.value, 'hello');
 assert.equal(h.el('#preview-matches').textContent, 'Match 2 of 2');
 assert.equal(h.el('#preview-content').scrollLeft, 100);
 assert.equal(reload.disabled, false);

 reload.handlers.click();
 picker.files = [{...file('root/src/App.tsx', ''), arrayBuffer: async () => { throw Error('Disk read failed.'); }}];
 await picker.handlers.change();
 assert.equal(source.textContent, 'hello hello\nnew');
 assert.equal(h.el('#preview-matches').textContent, 'Match 2 of 2');
 assert.match(h.el('#preview-panel-status').textContent, /Disk read failed.*previous snapshot/);

 reload.handlers.click();
 picker.files = [{...file('other/src/App.tsx', 'wrong'), arrayBuffer() { assert.fail('must not read a different folder'); }}];
 await picker.handlers.change();
 assert.equal(source.textContent, 'hello hello\nnew');
 assert.match(h.el('#preview-panel-status').textContent, /Reselect.*src\/App\.tsx/);
 h.el('#preview-clear').handlers.click();
 assert.equal(source.textContent, '');
});

test('browser blob reads are sliced to the byte cap plus one before materializing data', async () => {
 const h = setup();
 const source = file('root/source.md', 'source');
 source.arrayBuffer = () => assert.fail('unbounded arrayBuffer used directly');
 source.slice = (start, end) => {
  assert.equal(start, 0);
  assert.equal(end, 256 * 1024 + 1);
  return {arrayBuffer: async () => bytes('source')};
 };
 h.select([source]);
 await h.show('source.md');
 assert.equal(h.shown.at(-1).text, 'source');
});
