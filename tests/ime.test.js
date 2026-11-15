import test from 'node:test';import assert from 'node:assert/strict';import {installIme} from '../web/ime.js';
function setup(){const events=new Map(),sent=[];let epoch=1;const el={value:'',addEventListener(n,f){if(!events.has(n))events.set(n,[]);events.get(n).push(f)}};const ime=installIme(el,{getEpoch:()=>epoch,commit:(text,owner)=>sent.push([owner,text])});return {el,ime,sent,setEpoch:n=>epoch=n,event:(n,e={})=>{for(const fn of events.get(n)||[])fn(e)}};}
test('Chinese preedit never submits; end then final input commits exactly once',()=>{
 const h=setup();h.event('compositionstart');h.el.value='zhong';h.event('input',{data:'zhong',isComposing:true});assert.deepEqual(h.sent,[]);
 h.event('compositionend',{data:'中文'});h.event('input',{data:'中文',inputType:'insertText'});assert.deepEqual(h.sent,[[1,'中文']]);assert.equal(h.el.value,'');
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
