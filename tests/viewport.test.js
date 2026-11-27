import {backingGeometry} from '../web/frames.js';
import test from 'node:test';import assert from 'node:assert/strict';
import {dimensions,resizeQueue,cursorTarget,viewportDimensions,windowPixelQueue} from '../web/viewport.js';
const tick=()=>new Promise(r=>setImmediate(r));
test('sizes reject zero/fraction/overflow and preserve exact legal endpoints',()=>{for(const [rows,cols] of [[0,1],[1,0],[129,1],[1,257],[2.5,2],[NaN,2],[1,Infinity]])assert.throws(()=>dimensions(rows,cols));assert.deepEqual(dimensions(1,1),{rows:1,cols:1});assert.deepEqual(dimensions(128,256),{rows:128,cols:256});});
test('resize pressure retains at most active plus newest pending request',async()=>{let resolve;const calls=[],done=[];const queue=resizeQueue({getEpoch:()=>1,invoke:(_,a)=>{calls.push(a);return new Promise(r=>resolve=r);},onDone:s=>done.push(s),onError:assert.fail});const first=queue.request(2,3);for(let n=4;n<100;n++)queue.request(2,n);assert.equal(calls.length,1);resolve();await tick();assert.equal(calls.length,2);assert.equal(calls[1].cols,99);resolve();await first;assert.deepEqual(done,[{rows:2,cols:99}]);});
test('old resize epoch cannot block or report into new session',async()=>{let epoch=1;const pending=[],done=[],errors=[];const queue=resizeQueue({getEpoch:()=>epoch,invoke:(_,a)=>new Promise((resolve,reject)=>pending.push({a,resolve,reject})),onDone:s=>done.push(s),onError:e=>errors.push(e)});const old=queue.request(2,3);epoch=2;const fresh=queue.request(4,5);pending[1].resolve();await fresh;pending[0].reject(Error('old'));await old;assert.deepEqual(done,[{rows:4,cols:5}]);assert.deepEqual(errors,[]);});
test('resize failure without newer intent stops instead of retrying uncertain kernel operations',async()=>{let reject;let calls=0;const errors=[];const queue=resizeQueue({getEpoch:()=>1,invoke:()=>{calls++;return new Promise((_,r)=>reject=r)},onDone:assert.fail,onError:e=>errors.push(e)});const p=queue.request(2,3);reject(Error('ioctl'));await p;assert.equal(calls,1);assert.equal(errors.length,1);});
test('show cursor target uses canvas offset inside positioned scroll viewport and clamps edges',()=>{assert.deepEqual(cursorTarget({cursor_col:0,cursor_row:0},{offsetLeft:12,offsetTop:40},{clientWidth:100,clientHeight:100,scrollWidth:400,scrollHeight:400}),{left:0,top:0});assert.deepEqual(cursorTarget({cursor_col:255,cursor_row:127},{offsetLeft:12,offsetTop:40},{clientWidth:100,clientHeight:100,scrollWidth:400,scrollHeight:400}),{left:300,top:300});});

test('viewport fitting excludes borders, padding and its reserved vertical scrollbar',()=>{
 const viewport={offsetWidth:1001,clientWidth:984,clientHeight:393,getBoundingClientRect:()=>({width:1001,height:410})};
 const style={borderLeftWidth:'1px',borderRightWidth:'1px',borderTopWidth:'1px',borderBottomWidth:'1px',paddingLeft:'12px',paddingRight:'12px',paddingTop:'12px',paddingBottom:'12px'};
 assert.deepEqual(viewportDimensions(viewport,style),{rows:16,cols:80});
 // Transient horizontal overflow from the previous grid does not remove a row.
 viewport.clientHeight=408;
 assert.deepEqual(viewportDimensions(viewport,style),{rows:16,cols:80});
});

test('viewport fitting clamps narrow and large grids and skips hidden geometry',()=>{
 const viewport=(width,height)=>({offsetWidth:width,clientWidth:width,getBoundingClientRect:()=>({width,height})});
 assert.deepEqual(viewportDimensions(viewport(4,8),{}),{rows:1,cols:1});
 assert.deepEqual(viewportDimensions(viewport(9000,9000),{}),{rows:128,cols:256});
 assert.equal(viewportDimensions(viewport(0,0),{}),null);
 assert.equal(viewportDimensions(viewport(120,240),undefined),null);
});

function deferred(){let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return {promise,resolve,reject};}
function fixture(){
 let epoch=1;const calls=[],reports=[],errors=[];
 const queue=resizeQueue({getEpoch:()=>epoch,invoke:(_,args)=>{const d=deferred();calls.push({...d,args});return d.promise;},
  onDone:size=>{const d=deferred();reports.push({...d,size});return d.promise;},onError:e=>errors.push(e.message)});
 return {queue,calls,reports,errors,setEpoch:e=>epoch=e};
}
for(const stage of ['IPC','onDone']){
 async function start(h){
  const first=h.queue.request(24,80);
  if(stage==='onDone'){h.calls[0].resolve();await tick();}
  return {first,fail:()=> (stage==='IPC'?h.calls[0]:h.reports[0]).reject(Error(stage))};
 }
 test(`resize ${stage} failure preserves latest 40x100 target without retrying 24x80`,async()=>{
  const h=fixture();const {first,fail}=await start(h);
  h.queue.request(30,90);h.queue.request(40,100);fail();await tick();
  assert.equal(h.calls.length,2);assert.deepEqual(h.calls[1].args,{epoch:1,rows:40,cols:100});
  assert.deepEqual(h.errors,[stage]);
  h.calls[1].resolve();await tick();h.reports.at(-1).resolve();await first;
  assert.equal(h.calls.length,2);assert.deepEqual(h.reports.at(-1).size,{rows:40,cols:100});
 });
 test(`resize ${stage} failure stops when latest target also fails and allows explicit retry`,async()=>{
  const h=fixture();const {first,fail}=await start(h);
  for(let i=0;i<200;i++)h.queue.request(40,100);
  fail();await tick();assert.equal(h.calls.length,2);
  h.calls[1].reject(Error('latest failed'));await first;await tick();
  assert.equal(h.calls.length,2);assert.deepEqual(h.errors,[stage,'latest failed']);
  const retry=h.queue.request(50,120);assert.equal(h.calls.length,3);
  h.calls[2].resolve();await tick();h.reports.at(-1).resolve();await retry;
 });
 test(`resize stale ${stage} failure cannot consume or report into new epoch`,async()=>{
  const h=fixture();const {first,fail}=await start(h);h.queue.request(40,100);
  h.setEpoch(2);const fresh=h.queue.request(50,120);h.queue.request(60,140);
  fail();await first;assert.equal(h.calls.length,2);assert.deepEqual(h.errors,[]);
  h.calls[1].resolve();await tick();
  assert.equal(h.calls.length,3);assert.deepEqual(h.calls[2].args,{epoch:2,rows:60,cols:140});
  h.calls[2].resolve();await tick();h.reports.at(-1).resolve();await fresh;
 });
}

test('window pixel calculations follow physical scaling and the canvas backing cap',()=>{
  const pixels=(rows,cols,scale)=>{
    const {width,height}=backingGeometry(rows,cols,scale);
    return [width,height];
  };
  assert.deepEqual(pixels(24,80,1),[960,576]);
  assert.deepEqual(pixels(12,84,2),[2016,576]);
  assert.deepEqual(pixels(12,84,1.25),[1260,360]);
  const [width,height]=pixels(128,256,4);
  assert.ok(width*height <= 16777216);
  assert.ok(width < 256*12*4);
});

test('window pixel updates coalesce per owner and a retired request cannot block the next owner',async()=>{
  const firstOwner={}, secondOwner={};
  let owner=firstOwner;
  const requests=[];
  const report=windowPixelQueue({getOwner:()=>owner,update:(attachment,geometry)=>{
    const next=deferred();
    requests.push({attachment,geometry,...next});
    return next.promise;
  }});
  const first=report(owner,1,24,80,960,576);
  report(owner,1,24,80,960,576);
  report(owner,1,24,80,1440,864);
  report(owner,1,24,80,1920,1152);
  assert.equal(requests.length,1);
  requests[0].resolve();
  await tick();
  assert.deepEqual(requests[1].geometry,{layout:1,rows:24,cols:80,pixels:{width:1920,height:1152}});
  report(owner,2,12,84,1008,288);
  owner=secondOwner;
  const second=report(owner,2,12,84,2016,576);
  assert.equal(requests[2].attachment,secondOwner);
  requests[2].resolve();
  await second;
  requests[1].reject(new Error('previous owner left'));
  await first;
  assert.equal(requests.length,3);
});
