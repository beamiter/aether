import test from 'node:test';import assert from 'node:assert/strict';import {installIme} from '../web/ime.js';
function setup(){const events=new Map(),sent=[];let epoch=1;const el={value:'',addEventListener(n,f){if(!events.has(n))events.set(n,[]);events.get(n).push(f)}};const ime=installIme(el,{getEpoch:()=>epoch,commit:(text,owner)=>sent.push([owner,text])});return {el,ime,sent,setEpoch:n=>epoch=n,event:(n,e={})=>{for(const fn of events.get(n)||[])fn(e)}};}
test('Chinese preedit never submits; end then final input commits exactly once',()=>{
 const h=setup();h.event('compositionstart');h.el.value='zhong';h.event('input',{data:'zhong',isComposing:true});assert.deepEqual(h.sent,[]);
 h.event('compositionend',{data:'中文'});h.event('input',{data:'中文',inputType:'insertText'});assert.deepEqual(h.sent,[[1,'中文']]);assert.equal(h.el.value,'');
});
test('the visible preedit is cleared when its owner changes before the late commit',()=>{
 const events=new Map(),rendered=[];let epoch=1;
 const field={value:'',addEventListener(name,listener){events.set(name,listener);}};
 installIme(field,{getEpoch:()=>epoch,commit:()=>assert.fail('stale composition committed'),
   preedit:text=>rendered.push(text)});
 events.get('compositionstart')();
 events.get('compositionupdate')({data:'ni'});
 epoch=2;
 events.get('compositionupdate')({data:'你'});
 events.get('compositionend')({data:'你'});
 assert.deepEqual(rendered,['','ni','','']);
});
test('a deferred compositionend is canceled by blur and ordered before fresh typing',()=>{
 const events=new Map(),callbacks=[],written=[];
 const field={value:'',addEventListener(name,listener){events.set(name,listener);}};
 installIme(field,{getEpoch:()=>1,commit:text=>written.push(text),afterEnd:callback=>callbacks.push(callback)});
 events.get('compositionstart')();
 events.get('compositionend')({data:'ni'});
 events.get('blur')();
 callbacks.shift()();
 assert.deepEqual(written,[]);
 events.get('compositionstart')();
 events.get('compositionend')({data:'你'});
 events.get('input')({inputType:'insertText',data:'你'});
 events.get('keydown')({key:'x'});
 events.get('input')({inputType:'insertText',data:'x'});
 callbacks.shift()();
 assert.deepEqual(written,['你','x']);
});
test('final input before compositionend also commits once',()=>{
 const h=setup();h.event('compositionstart');h.event('input',{data:'你好',inputType:'insertFromComposition',isComposing:false});h.event('compositionend',{data:'你好'});assert.deepEqual(h.sent,[[1,'你好']]);
});
test('canceled/blurred composition and stale epoch do not commit',()=>{
 const h=setup();h.event('compositionstart');h.event('compositionend',{data:''});assert.deepEqual(h.sent,[]);
 h.event('compositionstart');h.setEpoch(2);h.event('compositionend',{data:'旧'});h.event('input',{data:'旧',inputType:'insertText'});assert.deepEqual(h.sent,[]);
 h.event('compositionstart');h.event('blur');h.event('compositionend',{data:'取消'});h.event('input',{data:'取消',inputType:'insertText'});assert.deepEqual(h.sent,[]);
});
test('reset rejects late commit; next intentional key accepts identical text',()=>{
 const h=setup();h.event('compositionstart');h.ime.reset();h.setEpoch(2);h.event('compositionend',{data:'中'});h.event('input',{data:'中',inputType:'insertText'});assert.deepEqual(h.sent,[]);
 h.event('keydown',{key:'a'});h.event('input',{data:'中',inputType:'insertText'});assert.deepEqual(h.sent,[[2,'中']]);
});
test('next ordinary key is not mistaken for duplicate composition commit',()=>{
 const h=setup();h.event('compositionstart');h.event('compositionend',{data:'a'});h.event('keydown',{key:'a'});h.event('input',{data:'a',inputType:'insertText'});assert.deepEqual(h.sent,[[1,'a'],[1,'a']]);
});

test('clipboard, drop, replacement and history edits cannot bypass paste review',()=>{
 for(const inputType of ['insertFromPaste','insertFromPasteAsQuotation','insertFromDrop','insertReplacementText','historyUndo','historyRedo','insertLineBreak','insertParagraph',undefined]){
  for(const data of ['echo first\necho second',null]){
   const h=setup();h.el.value='echo first\necho second';
   h.event('input',{inputType,data});
   assert.deepEqual(h.sent,[],String(inputType));assert.equal(h.el.value,'');
   h.event('keydown',{key:'a'});h.event('input',{inputType:'insertText',data:'a'});
   assert.deepEqual(h.sent,[[1,'a']]);
  }
 }
});
test('ordinary typed Unicode and null-data text fallback remain exact',()=>{
 const h=setup();h.event('input',{inputType:'insertText',data:'é'});
 h.el.value='👩‍💻';h.event('input',{inputType:'insertText',data:null});
 assert.deepEqual(h.sent,[[1,'é'],[1,'👩‍💻']]);assert.equal(h.el.value,'');
});
test('unrelated browser edit cannot consume composition duplicate suppression',()=>{
 const h=setup();h.event('compositionstart');h.event('compositionend',{data:'中'});
 h.event('input',{inputType:'insertFromDrop',data:'unreviewed\n'});
 h.event('input',{inputType:'insertText',data:'中'});
 assert.deepEqual(h.sent,[[1,'中']]);
});

for(const text of ['x'.repeat(1000000),'😀'.repeat(2049)]){
 test(`oversized composition quarantines repeated final input (${text.length} units)`,()=>{
  const h=setup();h.event('compositionstart');h.event('compositionend',{data:text});
  // The consumer receives the first commit to report its existing byte-bound error.
  assert.equal(h.sent.length,1);
  for(let i=0;i<3;i++)h.event('input',{inputType:'insertText',data:text});
  h.el.value=text;h.event('input',{inputType:'insertText',data:null});
  assert.equal(h.sent.length,1);assert.equal(h.el.value,'');
  h.event('keydown',{key:'Process'});h.event('input',{inputType:'insertText',data:text});
  assert.equal(h.sent.length,1);
  h.event('keydown',{key:'a'});h.event('input',{inputType:'insertText',data:'a'});
  assert.deepEqual(h.sent,[[1,text],[1,'a']]);
 });
}
test('oversized composition quarantine clears for a new composition',()=>{
 const h=setup();h.event('compositionstart');h.event('compositionend',{data:'x'.repeat(4097)});
 h.event('compositionstart');h.event('compositionend',{data:'中文'});
 h.event('input',{inputType:'insertText',data:'中文'});
 assert.equal(h.sent.length,2);assert.deepEqual(h.sent[1],[1,'中文']);
});
test('oversized composition cannot cross blur or epoch reset',()=>{
 for(const reset of [h=>h.event('blur'),h=>h.ime.reset()]){
  const h=setup(),text='x'.repeat(4097);
  h.event('compositionstart');h.event('compositionend',{data:text});reset(h);h.setEpoch(2);
  h.event('input',{inputType:'insertText',data:text});
  h.event('compositionend',{data:text});
  assert.equal(h.sent.length,1);
  h.event('keydown',{key:'b'});h.event('input',{inputType:'insertText',data:'b'});
  assert.deepEqual(h.sent[1],[2,'b']);
 }
});
test('maximum bounded composition still commits exactly once',()=>{
 for(const text of ['x'.repeat(4096),'😀'.repeat(1024)]){
  const h=setup();h.event('compositionstart');h.event('compositionend',{data:text});
  h.event('input',{inputType:'insertText',data:text});assert.deepEqual(h.sent,[[1,text]]);
 }
});


test('legacy IME 229 keydown preserves commit duplicate suppression',()=>{
 for(const key of ['Unidentified','Enter','a']){
  const h=setup();h.event('compositionstart');h.event('compositionend',{data:'中'});
  h.event('keydown',{key,keyCode:229,isComposing:false});
  h.event('input',{inputType:'insertText',data:'中'});
  assert.deepEqual(h.sent,[[1,'中']]);
  h.event('keydown',{key:'a',keyCode:65});h.event('input',{inputType:'insertText',data:'中'});
  assert.deepEqual(h.sent,[[1,'中'],[1,'中']]);
 }
});
test('legacy IME 229 cannot release stale or oversized composition quarantine',()=>{
 for(const stale of [true,false]){
  const h=setup(),text=stale?'旧':'x'.repeat(4097);
  h.event('compositionstart');
  if(stale)h.setEpoch(2);
  h.event('compositionend',{data:text});const before=h.sent.length;
  h.event('keydown',{key:'Unidentified',keyCode:229,isComposing:false});
  h.event('input',{inputType:'insertText',data:text});
  assert.equal(h.sent.length,before);
  h.event('keydown',{key:'b',keyCode:66});h.event('input',{inputType:'insertText',data:'b'});
  assert.deepEqual(h.sent.at(-1),[stale?2:1,'b']);
 }
});
test('already consumed keys preserve composition duplicate and stale-input guards',()=>{
 for(const stale of [false,true]){
  const h=setup();h.event('compositionstart');if(stale)h.setEpoch(2);h.event('compositionend',{data:'中'});const count=h.sent.length;
  h.event('keydown',{key:'Escape',defaultPrevented:true});h.event('input',{inputType:'insertText',data:'中'});assert.equal(h.sent.length,count);
  h.event('keydown',{key:'a'});h.event('input',{inputType:'insertText',data:'a'});assert.deepEqual(h.sent.at(-1),[stale?2:1,'a']);
 }
});
