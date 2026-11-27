import test from 'node:test';import assert from 'node:assert/strict';
import {acceptHistory,installHistory} from '../web/history.js';
import {CHANNEL_RENDITION_VERSION} from '../web/frames-v4.js';
import {packedRow} from './fixtures/packed-channel.js';
const page=(epoch=1,text='hello\nworld',offset=0,total_rows=3)=>({epoch,revision:'12',offset,total_rows,text});
const tick=()=>new Promise(r=>setImmediate(r));
function setup(){
 const element=()=>({value:'',textContent:'',handlers:{},setSelectionRange(){},addEventListener(n,f){this.handlers[n]=f}});
 let owner={session:'owner',epoch:1,generation:1};const requests=[];
 const items={older:element(),newer:element(),refresh:element(),output:element(),label:element()};
 const ui=installHistory({...items,getOwner:()=>owner,invoke:(_,args)=>new Promise((resolve,reject)=>
  requests.push({owner:args.owner,args:{epoch:args.owner.epoch,offset:args.offset},resolve,reject}))});
 return {...items,...ui,requests,setEpoch:epoch=>owner={...owner,epoch},
  rotate:()=>owner={...owner,generation:owner.generation+1},click:n=>items[n].handlers.click()};
}
test('history rejects foreign, unbounded and control-bearing snapshots',()=>{
 assert.equal(acceptHistory(page(),1).text,'hello\nworld');
 for(const change of [{epoch:2},{total_rows:1025},{offset:4},{text:'\x1b[31m'},{text:'x'.repeat(131201)},{revision:'-1'}])assert.throws(()=>acceptHistory({...page(),...change},1));
});
test('byte-limited history pages report their exact ranges and reach the oldest retained row',async()=>{
 const h=setup();
 for(const row_count of [0,0.5,129,null])assert.throws(()=>acceptHistory({...page(),row_count},1),/invalid history snapshot/);
 const first=h.click('refresh');h.requests[0].resolve({...page(1,'newest',0,200),row_count:32});await first;
 assert.match(h.label.textContent,/archived rows 169–200 of 200/);
 const older=h.click('older');for(let i=0;i<20;i++)h.click('older');
 h.requests[1].resolve({...page(1,'intermediate',20,200),row_count:32});await tick();
 assert.equal(h.requests[2].args.offset,168);
 h.requests[2].resolve({...page(1,'oldest',168,200),row_count:32});await older;
 assert.equal(h.output.value,'oldest');assert.match(h.label.textContent,/archived rows 1–32 of 200/);
 await h.click('older');assert.equal(h.requests.length,3);
});
test('history navigation steps through dense windows without skipping archived rows',async()=>{
 const h=setup();
 const first=h.click('refresh');h.requests[0].resolve({...page(1,'newest',0,60),row_count:8});await first;
 const older=h.click('older');assert.equal(h.requests[1].args.offset,8);
 h.requests[1].resolve({...page(1,'older',8,60),row_count:4});await older;
 const next=h.click('older');assert.equal(h.requests[2].args.offset,12);
 h.requests[2].resolve({...page(1,'oldest in view',12,60),row_count:4});await next;
 assert.equal(h.output.value,'oldest in view');
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

test('retired same-epoch history replies cannot alter selection, encode text, or correct a page',async()=>{
 const h=setup(), selectionWrites=[], encoded=[], Encoder=globalThis.TextEncoder;
 h.output.setSelectionRange=(...range)=>selectionWrites.push(range);
 globalThis.TextEncoder=class extends Encoder{encode(text){encoded.push(text);return super.encode(text);}};
 try {
  const committed=h.click('refresh');h.requests[0].resolve(page(1,'retained\u00a0text',0,300));await committed;
  const originalLabel=h.label.textContent;
  const stale=h.click('older');
  h.rotate();
  h.requests[1].resolve(page(1,'x'.repeat(131200),20,21));
  await tick();
  assert.equal(h.requests.length,2,'superseded owner cannot schedule an oldest-window correction');
  await stale;
  assert.deepEqual(encoded,['retained\u00a0text']);
  assert.equal(h.output.value,'retained\u00a0text');
  assert.equal(h.label.textContent,originalLabel);
  assert.deepEqual(selectionWrites,[[0,0]]);
  const fresh=h.click('older');
  assert.equal(h.requests[2].args.offset,20);
  assert.equal(h.requests[2].owner.generation,2);
  h.requests[2].resolve(page(1,'new owner',20,300));await fresh;
  assert.equal(h.output.value,'new owner');
 }finally{globalThis.TextEncoder=Encoder;}
});

test('retired same-epoch history failure does not surface an old error or consume queued navigation',async()=>{
 const h=setup();
 const committed=h.click('refresh');h.requests[0].resolve(page(1,'retained',0,300));await committed;
 const originalLabel=h.label.textContent;
 const stale=h.click('older');
 h.click('older');h.rotate();
 const fresh=h.click('older');
 assert.equal(h.requests.length,3,'retired group must not hold the current intent');
 assert.equal(h.requests[2].owner.generation,2);
 assert.equal(h.requests[2].args.offset,20,'retired pending intent is not inherited');
 h.requests[1].reject(Error('retired connection rejected'));
 await stale;
 assert.equal(h.requests.length,3);
 assert.equal(h.label.textContent,originalLabel);
 assert.equal(h.output.value,'retained');
 h.requests[2].resolve(page(1,'fresh',20,300));await fresh;
 assert.equal(h.output.value,'fresh');
});

function searchSetup(){
 const element=()=>({value:'',textContent:'',handlers:{},focus(){},
  setSelectionRange(start,end){this.selection=[start,end];},addEventListener(n,f){this.handlers[n]=f;}});
 let owner={session:'owner',epoch:1,generation:1};
 const requests=[];
 const controls={form:element(),query:element(),find:element(),older:element(),cancel:element(),status:element(),enabled:()=>true};
 const output=element(), older=element();
 output.focus=()=>{output.focusCount=(output.focusCount??0)+1;};
 const history=installHistory({output,label:element(),older,newer:element(),refresh:element(),searchControls:controls,
  getOwner:()=>owner,invoke:(name,args)=>new Promise((resolve,reject)=>requests.push({name,args,resolve,reject}))});
 return {controls,output,requests,dismiss:history.dismiss,olderPage:()=>older.handlers.click(),
  rotate:()=>{owner={...owner,generation:owner.generation+1};history.updateSearchControls();},
  find:query=>{controls.query.value=query;return controls.form.handlers.submit({preventDefault(){}});}};
}
test('native search sends one bounded step at a time and selects a validated UTF-16 range',async()=>{
 const h=searchSetup(),pending=h.find('中');
 assert.equal(h.requests[0].name,'search_local_history');
 assert.deepEqual(h.requests[0].args.search,{id:1,query:'中',restart:true});
 h.requests[0].resolve({kind:'more',id:1,epoch:1});await tick();
 assert.deepEqual(h.requests[1].args.search,{id:1,query:'中',restart:false});
 h.requests[1].resolve({kind:'match',id:1,epoch:1,snapshot:page(1,'🐍 中'),start:3,end:4});
 await pending;
 assert.deepEqual(h.output.selection,[3,4]);assert.equal(h.controls.older.disabled,false);
 const next=h.controls.older.handlers.click();
 h.requests[2].reject(Error('History changed. Start a new search.'));await next;
 assert.match(h.controls.status.textContent,/History changed/);assert.deepEqual(h.output.selection,[3,4]);
 assert.equal(h.controls.older.disabled,true);
});
test('a matched history window supplies the next ordinary page offset',async()=>{
 const h=searchSetup(), pending=h.find('中');
 h.requests[0].resolve({kind:'match',id:1,epoch:1,snapshot:{...page(1,'这里 中',10,200),row_count:4},start:3,end:4});
 await pending;
 const older=h.olderPage();assert.equal(h.requests[1].name,'history_shell');assert.equal(h.requests[1].args.offset,14);
 h.requests[1].resolve({...page(1,'older',14,200),row_count:4});await older;
 assert.equal(h.output.value,'older');
});
test('dismissed history ignores a late match even after reopening on the same pane',async()=>{
 const h=searchSetup();
 const stale=h.find('old');
 h.dismiss();
 const resumed=h.find('new');
 h.requests[0].resolve({kind:'match',id:1,epoch:1,snapshot:page(1,'old'),start:0,end:3});
 await stale;
 assert.equal(h.output.value,'');
 assert.equal(h.output.focusCount,undefined);
 h.requests[1].resolve({kind:'match',id:2,epoch:1,snapshot:page(1,'new'),start:0,end:3});
 await resumed;
 assert.equal(h.output.value,'new');
 assert.equal(h.output.focusCount,1);
});

test('cancel and an attachment change leave no stale search continuation or selection',async()=>{
 const h=searchSetup();
 const canceled=h.find('old');
 h.controls.cancel.handlers.click();
 h.requests[0].resolve({kind:'more',id:1,epoch:1});await canceled;
 assert.equal(h.requests.length,1);
 const retired=h.find('old');h.rotate();
 const fresh=h.find('fresh');
 h.requests[1].resolve({kind:'match',id:2,epoch:1,snapshot:page(1,'old'),start:0,end:3});
 await retired;
 assert.equal(h.output.value,'');
 assert.equal(h.requests[2].args.owner.generation,2);
 h.requests[2].resolve({kind:'match',id:3,epoch:1,snapshot:page(1,'fresh'),start:0,end:5});await fresh;
 assert.equal(h.output.value,'fresh');
});

test('a removed projected source discards both its displayed selection and superseded replies', async()=>{
 const element=()=>({value:'',textContent:'',handlers:{},style:{},dataset:{},
  setSelectionRange(start,end){this.selectionStart=start;this.selectionEnd=end;},
  addEventListener(name,handle){this.handlers[name]=handle;}});
 const owner={session:'owner',epoch:1,generation:2},requests=[];
 const output=element(),label=element(),refresh=element(),viewport={...element(),scrollTop:0,clientHeight:100};
 const context={setTransform(){},clearRect(){},save(){},beginPath(){},rect(){},clip(){},fillText(){},restore(){},fillRect(){}};
 const canvas={...element(),getContext:()=>context};
 const ui=installHistory({older:element(),newer:element(),refresh,output,label,getOwner:()=>owner,
  visual:{viewport,spacer:element(),canvas},enableProjection:()=>true,
  invoke:(_,args)=>new Promise(resolve=>requests.push({args,resolve}))});
 const projection=(row,text)=>JSON.stringify({owner,version:CHANNEL_RENDITION_VERSION,revision:'19',cols:1,offset:0,total_rows:1,text,
  rows:[{source:{row,column:0},text_offsets:[0,1],paint:packedRow({wrapped:false,cells:[
   {text,foreground:'#ffffff',background:'#000000',width:1,style:0}]})}]});
 const first=refresh.handlers.click();
 assert.equal(requests[0].args.frameVersion,CHANNEL_RENDITION_VERSION);
 requests[0].resolve(projection('10','Z'));await first;
 assert.equal(output.value,'Z');assert.equal(viewport.hidden,false);
 output.setSelectionRange(0,1);
 const pending=refresh.handlers.click();
 assert.deepEqual(requests[1].args.anchor,{row:'10',column:0});
 ui.updateRetained(owner,'11');
 assert.equal(output.value,'');assert.equal(viewport.hidden,true);
 requests[1].resolve(projection('10','Z'));await pending;
 assert.equal(output.value,'');assert.match(label.textContent,/History source expired/);
 const current=refresh.handlers.click();requests[2].resolve(projection('11','Q'));await current;
 assert.equal(output.value,'Q');assert.equal(viewport.hidden,false);
});

test('projected history preserves exact backward selection while reflowing overlapping pages',async()=>{
 const element=()=>({value:'',textContent:'',handlers:{},style:{},dataset:{},
  setSelectionRange(start,end,direction='none'){
   Object.assign(this,{selectionStart:start,selectionEnd:end,selectionDirection:direction});
  },addEventListener(name,handle){this.handlers[name]=handle;}});
 const owner={session:'owner',epoch:1,generation:2},requests=[];
 const output=element(),older=element(),refresh=element();
 const viewport={...element(),scrollTop:0,clientHeight:96};
 const context={setTransform(){},clearRect(){},save(){},beginPath(){},rect(){},clip(){},fillText(){},restore(){},fillRect(){}};
 installHistory({older,newer:element(),refresh,output,label:element(),getOwner:()=>owner,
  visual:{viewport,spacer:element(),canvas:{...element(),getContext:()=>context}},enableProjection:()=>true,
  invoke:(_,args)=>new Promise(resolve=>requests.push({args,resolve}))});
 function projection(parts,offset){
  let text='';
  const rows=parts.map(([row,column,glyphs,wrapped=false])=>{
   const text_offsets=[text.length],cells=glyphs.flatMap(glyph=>{
    text+=glyph;text_offsets.push(text.length);
    const cell={text:glyph,foreground:'#ffffff',background:'#000000',width:glyph==='中'?2:1,style:0};
    if(cell.width===1)return [cell];
    text_offsets.push(text.length);
    return [cell,{...cell,text:' ',width:0}];
   });
   if(!wrapped)text+='\n';
   return {source:{row:String(row),column},text_offsets,paint:packedRow({wrapped,cells})};
  });
  return JSON.stringify({owner,version:CHANNEL_RENDITION_VERSION,revision:'19',cols:8,offset,total_rows:100,rows,text:text.slice(0,-1)});
 }
 const first=refresh.handlers.click();
 requests[0].resolve(projection([[20,0,['A','中','B'],true],[20,4,['C']],[21,0,['D']]],0));
 await first;
 output.setSelectionRange(1,5,'backward');
 assert.equal(output.value.slice(output.selectionStart,output.selectionEnd),'中BC\n');

 const pending=older.handlers.click();
 assert.deepEqual(requests[1].args.anchor,{row:'20',column:0});
 requests[1].resolve(projection([[19,0,['X']],[20,0,['A','中'],true],[20,3,['B','C']],[21,0,['D']]],3));
 await pending;
 assert.equal(output.value,'X\nA中BC\nD');
 assert.equal(output.value.slice(output.selectionStart,output.selectionEnd),'中BC\n');
 assert.equal(output.selectionDirection,'backward');

 const replaced=refresh.handlers.click();
 requests[2].resolve(projection([[34,0,['A','中'],true],[34,3,['B','C']],[35,0,['D']]],3));
 await replaced;
 assert.equal(output.value,'A中BC\nD');
 assert.deepEqual([output.selectionStart,output.selectionEnd],[0,0]);
});
