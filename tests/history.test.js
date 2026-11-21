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
 h.requests[1].resolve(page(1,'',1024,1024));await tick();
 assert.equal(h.output.value,'selected');assert.equal(h.requests.length,3);assert.equal(h.requests[2].args.offset,896);
 h.requests[2].resolve(page(1,'final',896,1024));await pending;assert.equal(h.output.value,'final');assert.equal(h.requests.length,3);
});
test('rapid opposite navigation coalesces to the requested offset',async()=>{
 const h=setup();const pending=h.click('older');h.click('older');h.click('newer');
 h.requests[0].resolve(page(1,'old',20,200));await tick();assert.equal(h.requests[1].args.offset,20);
 h.requests[1].resolve(page(1,'latest',20,200));await pending;assert.equal(h.output.value,'latest');
});
test('history failure without newer intent preserves selected text and allows explicit retry',async()=>{
 const h=setup();h.output.value='selected';const pending=h.click('refresh');
 h.requests[0].reject(Error('transport lost'));await pending;
 assert.equal(h.requests.length,1);assert.equal(h.output.value,'selected');assert.match(h.label.textContent,/transport lost/);
 const retry=h.click('refresh');assert.equal(h.requests[1].args.offset,0);h.requests[1].resolve(page());await retry;
 assert.equal(h.output.value,'hello\nworld');
});
test('corrected oldest full window becomes the next navigation base',async()=>{
 const h=setup();const first=h.click('older');h.requests[0].resolve(page(1,'partial',20,130));await tick();
 assert.equal(h.requests[1].args.offset,2);h.requests[1].resolve(page(1,'oldest',2,130));await first;
 const next=h.click('newer');assert.equal(h.requests[2].args.offset,0);h.requests[2].resolve(page(1,'newest',0,130));await next;
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

for(const failure of ['reject','invalid']){
 const fail=request=>failure==='reject'?request.reject(Error('transport lost')):request.resolve(page(1,'\x1b'));
 test(`history ${failure} of superseded IPC preserves the newest navigation intent`,async()=>{
  const h=setup();h.output.value='selected';h.label.textContent='previous snapshot';
  const pending=h.click('older');h.click('older');h.click('newer');
  fail(h.requests[0]);await tick();
  assert.equal(h.requests.length,2);assert.equal(h.requests[1].args.offset,20);
  assert.equal(h.output.value,'selected');assert.equal(h.label.textContent,'previous snapshot');
  h.requests[1].resolve(page(1,'latest',20,200));await pending;
  assert.equal(h.output.value,'latest');assert.equal(h.requests.length,2);
 });
 test(`history ${failure} stops after the bounded latest target also fails`,async()=>{
  const h=setup();h.output.value='selected';const pending=h.click('refresh');
  for(let i=0;i<200;i++)h.click('older');
  fail(h.requests[0]);await tick();
  assert.equal(h.requests.length,2);assert.equal(h.requests[1].args.offset,1024);
  fail(h.requests[1]);await pending;await tick();
  assert.equal(h.requests.length,2);assert.equal(h.output.value,'selected');
  assert.match(h.label.textContent,/transport lost|invalid history snapshot/);
  const retry=h.click('older');assert.equal(h.requests[2].args.offset,20);
  h.requests[2].resolve(page(1,'retry',20,200));await retry;assert.equal(h.output.value,'retry');
 });
 test(`history stale ${failure} cannot dispatch queued work or alter a new epoch`,async()=>{
  const h=setup();const old=h.click('refresh');h.click('older');
  h.reset();h.setEpoch(2);const fresh=h.click('older');h.click('older');
  fail(h.requests[0]);await old;assert.equal(h.requests.length,2);
  assert.equal(h.output.value,'');assert.match(h.label.textContent,/Snapshot cleared/);
  h.requests[1].resolve(page(2,'intermediate',20,200));await tick();
  assert.equal(h.requests.length,3);assert.deepEqual(h.requests[2].args,{epoch:2,offset:40});
  h.requests[2].resolve(page(2,'fresh',40,200));await fresh;assert.equal(h.output.value,'fresh');
 });
}
test('history newer refresh is preserved even when its target equals the failed request',async()=>{
 const h=setup();const pending=h.click('refresh');h.click('refresh');
 h.requests[0].reject(Error('lost'));await tick();
 assert.equal(h.requests.length,2);assert.equal(h.requests[1].args.offset,0);
 h.requests[1].resolve(page());await pending;assert.equal(h.output.value,'hello\nworld');
});

test('unchanged page refresh preserves selected text while updating the revision label',async()=>{
 const h=setup(),selectionWrites=[];
 h.output.setSelectionRange=(...range)=>selectionWrites.push(range);
 const first=h.click('refresh');h.requests[0].resolve(page(1,'hello\nworld',0,150));await first;
 assert.deepEqual(selectionWrites,[[0,0]]);
 let rewrites=0,value=h.output.value;
 Object.defineProperty(h.output,'value',{get(){return value;},set(next){rewrites++;value=next;}});
 const same=h.click('refresh');h.requests[1].resolve({...page(1,'hello\nworld',0,150),revision:'13'});await same;
 assert.equal(rewrites,0);assert.deepEqual(selectionWrites,[[0,0]]);
 assert.match(h.label.textContent,/Snapshot 13/);

 const differentPage=h.click('older');h.requests[2].resolve(page(1,value,20,150));await differentPage;
 assert.equal(rewrites,1);assert.deepEqual(selectionWrites,[[0,0],[0,0]]);
 assert.match(h.label.textContent,/20 newer rows skipped/);
 const changed=h.click('refresh');h.requests[3].resolve(page(1,'updated text',20,150));await changed;
 assert.equal(rewrites,2);assert.equal(h.output.value,'updated text');
 assert.deepEqual(selectionWrites,[[0,0],[0,0],[0,0]]);
});

test('already superseded and previous-epoch history payloads skip validation entirely',async()=>{
 const h=setup(),encoded=[],Encoder=globalThis.TextEncoder;
 globalThis.TextEncoder=class extends Encoder{encode(text){encoded.push(text);return super.encode(text);}};
 try{
  const old=h.click('refresh');h.click('older');
  h.requests[0].resolve(page(1,'x'.repeat(131200)));await tick();
  assert.deepEqual(encoded,[]);assert.equal(h.requests.length,2);
  h.requests[1].resolve(page(1,'latest',20,200));await old;
  assert.deepEqual(encoded,['latest']);
  const previous=h.click('refresh');h.reset();h.setEpoch(2);
  const fresh=h.click('refresh');
  h.requests[2].resolve(page(1,'y'.repeat(131200)));await previous;
  assert.deepEqual(encoded,['latest']);assert.equal(h.output.value,'');
  h.requests[3].resolve(page(2,'new session'));await fresh;
  assert.deepEqual(encoded,['latest','new session']);assert.equal(h.output.value,'new session');
 }finally{globalThis.TextEncoder=Encoder;}
});

// Mirrors the Rust bridge's 128-row end-relative window including empty overshoot.
function rustReply(request,total,revision='12'){
 const offset=Math.min(request.args.offset,total),end=total-offset,start=Math.max(0,end-128);
 const text=Array.from({length:end-start},(_,i)=>'archived '+(start+i+1)).join('\n');
 request.resolve({...page(request.args.epoch,text,offset,total),revision});
}
test('three-row archive: older is harmless after Refresh, and first Older corrects once',async()=>{
 for(const initial of ['refresh','older']){
  const h=setup();let selections=0;h.output.setSelectionRange=()=>selections++;
  const first=h.click(initial);rustReply(h.requests[0],3);
  if(initial==='older'){
   await tick();assert.equal(h.output.value,'');assert.equal(selections,0);
   assert.equal(h.requests[1].args.offset,0);rustReply(h.requests[1],3);
  }
  await first;assert.equal(h.output.value,'archived 1\narchived 2\narchived 3');
  assert.equal(selections,1);assert.match(h.label.textContent,/archived rows 1–3 of 3/);
  const count=h.requests.length;
  const noOlder=h.click('older');assert.equal(h.requests.length,count);await noOlder;
  const noNewer=h.click('newer');assert.equal(h.requests.length,count);await noNewer;
  assert.equal(selections,1);
 }
});
test('archive edges keep all 128 displayed rows and still allow explicit refresh',async()=>{
 const h=setup();const first=h.click('refresh');rustReply(h.requests[0],129);await first;
 const older=h.click('older');assert.equal(h.requests[1].args.offset,1);
 rustReply(h.requests[1],129);await older;
 assert.match(h.label.textContent,/archived rows 1–128 of 129/);
 await h.click('older');assert.equal(h.requests.length,2);
 const refresh=h.click('refresh');rustReply(h.requests[2],130);await refresh;
 const next=h.click('older');assert.equal(h.requests[3].args.offset,2);
 rustReply(h.requests[3],130);await next;
 assert.match(h.label.textContent,/archived rows 1–128 of 130/);
});
test('archive shrink preserves selected page until valid correction, including a genuinely empty archive',async()=>{
 const h=setup();const first=h.click('refresh');rustReply(h.requests[0],400);await first;
 let selectionWrites=0;h.output.setSelectionRange=()=>selectionWrites++;
 const previous=h.output.value,label=h.label.textContent;
 const older=h.click('older');rustReply(h.requests[1],3);await tick();
 assert.equal(h.output.value,previous);assert.equal(h.label.textContent,label);assert.equal(selectionWrites,0);
 assert.equal(h.requests[2].args.offset,0);
 rustReply(h.requests[2],0);await older;
 assert.equal(h.output.value,'');assert.match(h.label.textContent,/no archived rows/);assert.equal(selectionWrites,1);
});
test('at most one correction when the archive shrinks twice; then preserve prior text and explicit refresh',async()=>{
 const h=setup();const first=h.click('refresh');rustReply(h.requests[0],400);await first;
 const previous=h.output.value;
 const older=h.click('older');rustReply(h.requests[1],130);await tick();assert.equal(h.requests[2].args.offset,2);
 rustReply(h.requests[2],129);await older;
 assert.equal(h.requests.length,3);assert.equal(h.output.value,previous);
 assert.match(h.label.textContent,/Archive changed during navigation/);
 const refresh=h.click('refresh');assert.equal(h.requests[3].args.offset,0);
 rustReply(h.requests[3],129);await refresh;assert.match(h.label.textContent,/archived rows 2–129/);
});
test('new navigation supersedes an already in-flight correction without a stale selection change',async()=>{
 const h=setup();const first=h.click('refresh');rustReply(h.requests[0],400);await first;
 const previous=h.output.value;
 const older=h.click('older');rustReply(h.requests[1],133);await tick();assert.equal(h.requests[2].args.offset,5);
 h.click('newer');rustReply(h.requests[2],133);await tick();
 assert.equal(h.requests.length,4);assert.equal(h.requests[3].args.offset,0);
 assert.equal(h.output.value,previous);
 rustReply(h.requests[3],133);await older;assert.match(h.label.textContent,/archived rows 6–133/);
});
test('old-epoch correction cannot overwrite the new shell or dispatch again',async()=>{
 const h=setup();const first=h.click('older');rustReply(h.requests[0],3);await tick();
 assert.equal(h.requests[1].args.offset,0);
 h.reset();h.setEpoch(2);
 const fresh=h.click('refresh');rustReply(h.requests[2],7);await fresh;
 const freshText=h.output.value,freshLabel=h.label.textContent;
 rustReply(h.requests[1],3);await first;
 assert.equal(h.requests.length,3);assert.equal(h.output.value,freshText);assert.equal(h.label.textContent,freshLabel);
});
