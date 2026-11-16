import test from 'node:test';import assert from 'node:assert/strict';
import {acceptHistory,installHistory} from '../web/history.js';
const page=(epoch=1,text='hello\nworld',offset=0,total_rows=3)=>({epoch,revision:'12',offset,total_rows,text});
const tick=()=>new Promise(r=>setImmediate(r));
function setup(){
 const element=()=>({value:'',textContent:'',handlers:{},setSelectionRange(){},addEventListener(n,f){this.handlers[n]=f}});
 let epoch=1;const requests=[];
 const items={older:element(),newer:element(),refresh:element(),output:element(),label:element()};
 const ui=installHistory({...items,getEpoch:()=>epoch,invoke:(_,args)=>new Promise((resolve,reject)=>requests.push({args,resolve,reject}))});
 return {...items,...ui,requests,setEpoch:e=>epoch=e,click:n=>items[n].handlers.click()};
}
test('history rejects foreign, unbounded and control-bearing snapshots',()=>{
 assert.equal(acceptHistory(page(),1).text,'hello\nworld');
 for(const change of [{epoch:2},{total_rows:1025},{offset:4},{text:'\x1b[31m'},{text:'x'.repeat(131201)},{revision:'-1'}])assert.throws(()=>acceptHistory({...page(),...change},1));
});
test('history reset discards old completion and a stuck epoch does not block a new one',async()=>{
 const h=setup();const old=h.click('refresh');h.reset();h.setEpoch(2);const fresh=h.click('refresh');
 assert.equal(h.requests.length,2);h.requests[0].resolve(page(1,'stale'));await old;assert.equal(h.output.value,'');
 h.requests[1].resolve(page(2,'new'));await fresh;assert.equal(h.output.value,'new');
});
test('history burst keeps one active and one latest target, not hundreds of IPCs',async()=>{
 const h=setup();h.output.value='selected';const pending=h.click('refresh');
 for(let i=0;i<200;i++)h.click('older');
 assert.equal(h.requests.length,1);
 h.requests[0].resolve(page(1,'intermediate'));await tick();
 assert.equal(h.output.value,'selected');assert.equal(h.requests.length,2);assert.equal(h.requests[1].args.offset,1024);
 h.requests[1].resolve(page(1,'final',1024,1024));await pending;assert.equal(h.output.value,'final');assert.equal(h.requests.length,2);
});
test('rapid opposite navigation coalesces to the requested offset',async()=>{
 const h=setup();const pending=h.click('older');h.click('older');h.click('newer');
 h.requests[0].resolve(page(1,'old',20,200));await tick();assert.equal(h.requests[1].args.offset,20);
 h.requests[1].resolve(page(1,'latest',20,200));await pending;assert.equal(h.output.value,'latest');
});
test('history failure preserves selected text, drops queued request and allows explicit retry',async()=>{
 const h=setup();h.output.value='selected';const pending=h.click('refresh');h.click('older');
 h.requests[0].reject(Error('transport lost'));await pending;
 assert.equal(h.requests.length,1);assert.equal(h.output.value,'selected');assert.match(h.label.textContent,/transport lost/);
 const retry=h.click('refresh');assert.equal(h.requests[1].args.offset,0);h.requests[1].resolve(page());await retry;
 assert.equal(h.output.value,'hello\nworld');
});
test('clamped backend offset becomes the next navigation base',async()=>{
 const h=setup();const first=h.click('older');h.requests[0].resolve(page(1,'short',3,3));await first;
 const next=h.click('newer');assert.equal(h.requests[1].args.offset,0);h.requests[1].resolve(page());await next;
});

test('history rejects oversized strings before encoding and keeps exact UTF-8 limits',()=>{
 const Encoder=globalThis.TextEncoder;const encoded=[];
 globalThis.TextEncoder=class extends Encoder{encode(text){encoded.push(text.length);return super.encode(text)}};
 try {
  assert.throws(()=>acceptHistory(page(1,'x'.repeat(1000000)),1),/invalid history snapshot/);
  assert.deepEqual(encoded,[]);
  for(const text of ['x'.repeat(131200),'é'.repeat(65600),'😀'.repeat(32800)])
   assert.equal(acceptHistory(page(1,text),1).text,text);
  for(const text of ['é'.repeat(65600)+'x','😀'.repeat(32800)+'x','\ud800'.repeat(43734)])
   assert.throws(()=>acceptHistory(page(1,text),1),/invalid history snapshot/);
  assert.ok(encoded.every(length=>length<=131200));
 }finally{globalThis.TextEncoder=Encoder;}
});
test('oversized history response preserves selected text and permits a valid refresh',async()=>{
 const h=setup();h.output.value='selected';const first=h.click('refresh');
 h.requests[0].resolve(page(1,'x'.repeat(1000000)));await first;
 assert.equal(h.output.value,'selected');assert.match(h.label.textContent,/invalid history snapshot/);
 const retry=h.click('refresh');h.requests[1].resolve(page(1,'valid'));await retry;
 assert.equal(h.output.value,'valid');
});
