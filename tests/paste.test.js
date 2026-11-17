import test from 'node:test';import assert from 'node:assert/strict';
import {installPasteReview} from '../web/paste.js';
function harness(){
 const el=()=>({value:'',hidden:false,disabled:false,handlers:{},addEventListener(n,fn){this.handlers[n]=fn;}});
 const panel=el(),preview=el(),escaped=el(),confirm=el(),cancel=el(),sent=[],errors=[];let epoch=1;
 const review=installPasteReview({panel,preview,escaped,confirm,cancel,getEpoch:()=>epoch,send:(text,owner)=>sent.push({text,owner}),onError:m=>errors.push(m)});
 return {panel,preview,escaped,confirm,cancel,sent,errors,...review,setEpoch:value=>epoch=value};
}
test('single-line paste remains direct and exact, including tabs and Unicode',()=>{
 const h=harness();h.offer('echo\t你好');assert.deepEqual(h.sent,[{text:'echo\t你好',owner:1}]);assert.equal(h.panel.hidden,true);
});
test('LF CR and CRLF require original escaped preview and explicit one-shot confirm',()=>{
 for(const text of ['echo a\necho b','echo a\r','echo a\r\necho b']){
  const h=harness();h.offer(text);assert.equal(h.sent.length,0);assert.equal(h.escaped.textContent,JSON.stringify(text));assert.equal(h.panel.hidden,false);
  h.confirm.handlers.click();h.confirm.handlers.click();assert.deepEqual(h.sent,[{text,owner:1}]);assert.equal(h.panel.hidden,true);assert.equal(h.preview.value,'');
 }
});
test('cancel and reset discard text without sending',()=>{
 const h=harness();h.offer('a\nb');h.cancel.handlers.click();h.confirm.handlers.click();h.offer('c\nd');h.reset();h.confirm.handlers.click();assert.deepEqual(h.sent,[]);
});
test('stale epoch, unavailable input, and closed session never receive reviewed text',()=>{
 for(const next of [2,null]){const h=harness();h.offer('a\nb');h.setEpoch(next);h.confirm.handlers.click();assert.deepEqual(h.sent,[]);assert.equal(h.panel.hidden,true);}
 const h=harness();h.setEpoch(null);h.offer('a');assert.deepEqual(h.sent,[]);
});
test('new clipboard offer replaces rather than appends pending review',()=>{
 const h=harness();h.offer('old\n');h.offer('new\n');h.confirm.handlers.click();assert.deepEqual(h.sent,[{text:'new\n',owner:1}]);
});
test('UTF-8 byte bound is exact and rejection clears old review',()=>{
 const h=harness();h.offer('é'.repeat(2047)+'\nX');assert.equal(h.panel.hidden,false);h.confirm.handlers.click();assert.equal(h.sent.length,1);
 h.offer('pending\n');h.offer('é'.repeat(2048)+'\n');h.confirm.handlers.click();assert.equal(h.sent.length,1);assert.equal(h.errors.length,1);
});
test('unsupported controls and empty clipboard never send or retain old review',()=>{
 for(const text of ['\x1b[31m','a\x00','a\x7f','a\x0b','']){const h=harness();h.offer('pending\n');h.offer(text);h.confirm.handlers.click();assert.deepEqual(h.sent,[]);assert.equal(h.panel.hidden,true);}
});

test('oversized clipboard rejects before encoding and clears pending review',()=>{
 const OriginalEncoder=globalThis.TextEncoder;let encodedUnits=[];
 globalThis.TextEncoder=class extends OriginalEncoder{encode(text){encodedUnits.push(text.length);assert.ok(text.length<=4096);return super.encode(text);}};
 try{
  for(const text of ['x'.repeat(1_000_000),'界'.repeat(4097),'😀'.repeat(2049)]){
   const h=harness();h.offer('pending\n');encodedUnits=[];h.offer(text);
   assert.deepEqual(encodedUnits,[]);assert.equal(h.errors.length,1);
   assert.equal(h.panel.hidden,true);assert.equal(h.preview.value,'');assert.equal(h.confirm.disabled,true);
   h.confirm.handlers.click();assert.deepEqual(h.sent,[]);
   h.offer('ok');assert.deepEqual(h.sent,[{text:'ok',owner:1}]);
  }
 }finally{globalThis.TextEncoder=OriginalEncoder;}
});
test('bounded encoding preserves exact ASCII astral and replacement byte limits',()=>{
 for(const text of ['x'.repeat(4096),'😀'.repeat(1024),'\ud800'.repeat(1365)+'x']){
  const h=harness();h.offer(text);assert.deepEqual(h.sent,[{text,owner:1}]);assert.deepEqual(h.errors,[]);
 }
 for(const text of ['x'.repeat(4097),'😀'.repeat(1024)+'x','\ud800'.repeat(1365)+'xx']){
  const h=harness();h.offer(text);assert.deepEqual(h.sent,[]);assert.equal(h.errors.length,1);
 }
});

test('browser-normalized preview cannot change original mixed line endings sent',()=>{
 const h=harness();let visual='';Object.defineProperty(h.preview,'value',{get:()=>visual,set:value=>{visual=value.replace(/\r\n?/g,'\n');}});
 for(const text of ['A\r\nB\rC\n','\r\r\n\n','literal \\r\t\"<b>界\r\n']){
  h.offer(text);assert.equal(h.preview.value,text.replace(/\r\n?/g,'\n'));
  assert.equal(JSON.parse(h.escaped.textContent),text);
  h.confirm.handlers.click();assert.equal(h.sent.at(-1).text,text);assert.equal(h.escaped.textContent,'');
 }
});
test('escaped original is cleared on cancellation invalid replacement and reset',()=>{
 const h=harness();
 for(const clear of [()=>h.cancel.handlers.click(),()=>h.reset(),()=>h.offer('x'.repeat(4097)),()=>h.offer(''),()=>h.offer('direct')]){
  h.offer('secret\r\n');assert.ok(h.escaped.textContent);clear();assert.equal(h.escaped.textContent,'');
 }
});

test('escaped view distinguishes literal escapes and remains bounded at input limit',()=>{
 const h=harness();h.offer('\\r\r\\n\n');assert.equal(h.escaped.textContent,JSON.stringify('\\r\r\\n\n'));
 const text='\t'.repeat(4095)+'\n';h.offer(text);assert.equal(JSON.parse(h.escaped.textContent),text);assert.ok(h.escaped.textContent.length<=6*4096+2);
 h.offer(text+'x');assert.equal(h.escaped.textContent,'');assert.equal(h.confirm.disabled,true);
});


test('format controls are visible in quoted review without changing multilingual payload',()=>{
 const formats=['\u061c','\u200e','\u200f','\u202a','\u202b','\u202c','\u202d','\u202e','\u2066','\u2067','\u2068','\u2069','\u200b','\u200c','\u200d','\u2060','\ufeff','\u{e0001}'];
 for(const format of formats){
  const h=harness(),text='中文 עברית العربية '+format+'text\r\n';h.offer(text);
  assert.equal(h.preview.value,text);assert.ok(!h.escaped.textContent.includes(format));
  assert.ok(h.escaped.textContent.includes('\\u'));assert.equal(JSON.parse(h.escaped.textContent),text);
  h.confirm.handlers.click();assert.deepEqual(h.sent,[{text,owner:1}]);
 }
});
test('literal format escapes remain distinct and normal multilingual text remains readable',()=>{
 const h=harness(),text='literal \\u202e actual \u202e 中文 עברית العربية 👩‍💻\n';h.offer(text);
 assert.ok(h.escaped.textContent.includes('literal \\\\u202e actual \\u202e'));
 assert.ok(h.escaped.textContent.includes('中文 עברית العربية'));assert.ok(h.escaped.textContent.includes('👩\\u200d💻'));
 assert.equal(JSON.parse(h.escaped.textContent),text);
});
test('format review still obeys input bound cancellation and stale ownership',()=>{
 const text='\u202e'.repeat(1365)+'\n',h=harness();h.offer(text);
 assert.equal(JSON.parse(h.escaped.textContent),text);assert.ok(h.escaped.textContent.length<=6*4096+2);
 h.cancel.handlers.click();assert.equal(h.escaped.textContent,'');h.confirm.handlers.click();assert.deepEqual(h.sent,[]);
 h.offer(text);h.setEpoch(2);h.confirm.handlers.click();assert.deepEqual(h.sent,[]);
 h.offer(text+'x');assert.equal(h.errors.length,1);assert.equal(h.escaped.textContent,'');
});
test('single-line format characters retain existing direct paste policy',()=>{
 const h=harness(),text='עברית\u202e\u200d';h.offer(text);assert.deepEqual(h.sent,[{text,owner:1}]);assert.equal(h.panel.hidden,true);
});

function focusHarness(){
 const h=harness(),document={activeElement:null};let focused=0;
 const origin={isConnected:true,focus(){focused++;document.activeElement=this;}};
 h.panel.ownerDocument=document;h.panel.contains=target=>[h.preview,h.escaped,h.confirm,h.cancel].includes(target);
 return {...h,document,origin,focused:()=>focused};
}
test('confirm and cancel restore the originating terminal focus only after explicit review action',()=>{
 for(const action of ['confirm','cancel']){
  const h=focusHarness();h.offer('review\n',h.origin);h.document.activeElement=h[action];h[action].handlers.click();
  assert.equal(h.focused(),1);assert.equal(h.document.activeElement,h.origin);assert.equal(h.sent.length,action==='confirm'?1:0);
  h[action].handlers.click();assert.equal(h.focused(),1);
 }
});
test('review never steals unrelated focus or focuses detached and stale session targets',()=>{
 for(const change of [h=>{h.document.activeElement={};},h=>{h.origin.isConnected=false;},h=>h.setEpoch(2),h=>h.setEpoch(null)]){
  const h=focusHarness();h.offer('review\n',h.origin);h.document.activeElement=h.confirm;change(h);h.confirm.handlers.click();assert.equal(h.focused(),0);
 }
 const h=focusHarness();h.offer('review\n',h.origin);h.document.activeElement=h.cancel;h.reset();assert.equal(h.focused(),0);h.cancel.handlers.click();assert.equal(h.focused(),0);
});
test('replacement review restores only its own origin and direct paste never changes focus',()=>{
 const h=focusHarness();let otherFocus=0;const other={isConnected:true,focus(){otherFocus++;}};
 h.offer('direct',h.origin);assert.equal(h.focused(),0);
 h.offer('old\n',h.origin);h.offer('new\n',other);h.document.activeElement=h.confirm;h.confirm.handlers.click();
 assert.equal(h.focused(),0);assert.equal(otherFocus,1);assert.equal(h.sent.at(-1).text,'new\n');
});

function reviewKey(h,extra={}){
 let prevented=0,stopped=0;
 h.panel.handlers.keydown({key:'Escape',target:h.preview,preventDefault(){prevented++;},stopPropagation(){stopped++;},...extra});
 return {prevented,stopped};
}
test('Escape cancels focused paste review once without sending and restores its origin',()=>{
 const h=focusHarness();h.offer('review\n',h.origin);h.document.activeElement=h.preview;
 assert.deepEqual(reviewKey(h),{prevented:1,stopped:1});
 assert.equal(h.panel.hidden,true);assert.equal(h.preview.value,'');assert.equal(h.escaped.textContent,'');
 assert.equal(h.focused(),1);h.confirm.handlers.click();assert.deepEqual(h.sent,[]);
 assert.deepEqual(reviewKey(h),{prevented:0,stopped:0});assert.equal(h.focused(),1);
});
test('review Escape leaves composing handled modified and outside key events alone',()=>{
 for(const extra of [{isComposing:true},{keyCode:229},{defaultPrevented:true},{ctrlKey:true},{altKey:true},{shiftKey:true},{metaKey:true},{key:'Enter'},{target:{}}]){
  const h=focusHarness();h.offer('review\n',h.origin);h.document.activeElement=h.preview;
  assert.deepEqual(reviewKey(h,extra),{prevented:0,stopped:0});assert.equal(h.panel.hidden,false);assert.equal(h.focused(),0);
  h.confirm.handlers.click();assert.deepEqual(h.sent,[{text:'review\n',owner:1}]);
 }
});
test('Escape discards stale review without moving focus to old session',()=>{
 for(const epoch of [2,null]){
  const h=focusHarness();h.offer('review\n',h.origin);h.document.activeElement=h.preview;h.setEpoch(epoch);
  assert.deepEqual(reviewKey(h),{prevented:1,stopped:1});assert.equal(h.panel.hidden,true);assert.equal(h.focused(),0);assert.deepEqual(h.sent,[]);
 }
});

