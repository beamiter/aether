import {installPasteReview} from '../web/paste.js';
import {dimensions,resizeQueue,cursorTarget,viewportDimensions} from '../web/viewport.js';
import test from 'node:test';import assert from 'node:assert/strict';import fs from 'node:fs';import vm from 'node:vm';
import {installHistory} from '../web/history.js';
import {acceptFull} from '../web/frames.js'; import {installIme} from '../web/ime.js';
import {installTerminalSelection} from '../web/terminal-selection.js';
import {installTerminalMouse} from '../web/terminal-mouse.js';
const tick=()=>new Promise(r=>setImmediate(r));
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return {promise,resolve,reject};};
function harness(rawInvoke, health=args=>({epoch:args.epoch,exited:false,unsupported:"0"}), Encoder=TextEncoder, viewportSize=null){
 const invoke=(name,args)=>name==='shell_status'?Promise.resolve(health(args)).then(s=>({drained:false,write_error:null,dropped_reply_bytes:'0',...s})):rawInvoke(name,args);
 const elements=new Map(),timers=new Map(),painted=[];let id=0;
 const el=selector=>{
  if(!elements.has(selector)){
   const listeners=new Map();
   elements.set(selector,{disabled:false,value:'',textContent:'',style:{},handlers:{},focus(){},select(){this.selected=true},replaceChildren(){},
    addEventListener(name,fn,options){
     const group=listeners.get(name)??[];
     group.push({fn,capture:options?.capture??false});
     group.sort((left,right)=>Number(right.capture)-Number(left.capture));
     listeners.set(name,group);
     this.handlers[name]=event=>{
      let result;
      for(const {fn} of group)result=fn(event)??result;
      return result;
     };
    }});
  }
  return elements.get(selector);
 };
 const context={installFilePreviewPanel:()=>({dismiss(){}}),document:{querySelector:el,createElement:()=>({style:{}})},__TAURI__:{core:{invoke}},acceptFull,installIme,installHistory,installPasteReview,installTerminalSelection,installTerminalMouse,dimensions,resizeQueue,cursorTarget,viewportDimensions,paint(_,frame){painted.push(frame.revision);},TextEncoder:Encoder,TextDecoder,
 setInterval(fn){timers.set(++id,fn);return id},clearInterval(i){timers.delete(i)},setTimeout};
 el('#fit-size').checked=true;
 let onViewportChange;
 function setViewport(cols,rows){
  const viewport=el('.viewport'),width=cols*12+26,height=rows*24+26;
  viewport.offsetWidth=width;viewport.clientWidth=width-2;
  viewport.getBoundingClientRect=()=>({width,height});
  onViewportChange?.();
 }
 if(viewportSize){
  context.getComputedStyle=()=>({borderLeftWidth:'1px',borderRightWidth:'1px',borderTopWidth:'1px',borderBottomWidth:'1px',paddingLeft:'12px',paddingRight:'12px',paddingTop:'12px',paddingBottom:'12px'});
  context.ResizeObserver=class{constructor(callback){onViewportChange=callback;}observe(node){assert.equal(node,el('.viewport'));}};
  setViewport(...viewportSize);
 }
 vm.runInNewContext(fs.readFileSync(new URL('../web/app.js',import.meta.url),'utf8').replace(/^import .*\n/gm,''),context);
 return {el,timers,painted,setViewport,click:s=>el(s).handlers.click(),submit:()=>el('#command').handlers.submit({preventDefault(){}})};
}

test('viewport fitting starts the PTY at the measured size and resumes after a fixed override',async()=>{
 const calls=[];
 const h=harness(async(name,args)=>{
  if(name==='open_shell'||name==='resize_shell')calls.push([name,{...args}]);
  return name==='open_shell'?1:null;
 },undefined,undefined,[70,16]);
 await h.click('#open');
 h.setViewport(74,18);h.setViewport(74,18);await tick();
 h.el('#cols').value='60';h.el('#rows').value='12';await h.click('#resize');
 assert.equal(h.el('#fit-size').checked,false);
 h.setViewport(78,20);await tick();
 h.el('#fit-size').checked=true;h.el('#fit-size').handlers.change();await tick();
 assert.deepEqual(calls,[
  ['open_shell',{cols:70,rows:16}],
  ['resize_shell',{epoch:1,cols:74,rows:18}],
  ['resize_shell',{epoch:1,cols:60,rows:12}],
  ['resize_shell',{epoch:1,cols:78,rows:20}],
 ]);
});

test('fitting follows geometry that changes during open and drops its queued resize when disabled',async()=>{
 const opening=deferred(),resize=deferred(),calls=[];
 const h=harness(async(name,args)=>{
  if(name==='open_shell'){calls.push({...args});return opening.promise;}
  if(name==='resize_shell'){calls.push({...args});return resize.promise;}
  return null;
 },undefined,undefined,[40,10]);
 const opened=h.click('#open');
 h.setViewport(45,11);
 opening.resolve(1);await opened;
 h.setViewport(50,12);
 h.el('#fit-size').checked=false;h.el('#fit-size').handlers.change();
 resize.resolve();await tick();
 assert.deepEqual(calls,[{cols:40,rows:10},{epoch:1,cols:45,rows:11}]);
});

test('invalid fixed size leaves fitting enabled and a terminated shell ignores viewport changes',async()=>{
 let exited=false;const resizes=[];
 const h=harness(async(name,args)=>{
  if(name==='resize_shell')resizes.push({...args});
  return name==='open_shell'?1:null;
 },args=>({epoch:args.epoch,exited,unsupported:'0'}),undefined,[70,16]);
 await h.click('#open');
 h.el('#cols').value='0';await h.click('#resize');
 assert.match(h.el('#status').textContent,/Use integer rows/);
 assert.equal(h.el('#fit-size').checked,true);
 exited=true;await [...h.timers.values()][0]();
 h.setViewport(74,18);await tick();
 assert.deepEqual(resizes,[]);
});
test('PTY ACKs continue while dragging, then copy uses the frame the pointer saw',async()=>{
 const first=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 first.session='pty-demo';
 const next=structuredClone(first);next.revision=2;
 next.rows[0][0].text='X';
 const pending=[first,next],acked=[];
 const h=harness(async(name,args)=>{
  if(name==='open_shell')return 1;
  if(name==='poll_shell')return pending.shift()??null;
  if(name==='ack_shell')acked.push(args.revision);
 });
 const canvas=h.el('canvas');
 let captured=null;
 canvas.setPointerCapture=id=>captured=id;
 canvas.hasPointerCapture=id=>captured===id;
 canvas.releasePointerCapture=()=>captured=null;
 canvas.getBoundingClientRect=()=>({left:0,top:0,width:first.cols*10,height:first.rows.length*10});
 const point=col=>({isPrimary:true,pointerType:'mouse',pointerId:1,button:0,clientX:col*10+5,clientY:5,preventDefault(){}});
 await h.click('#open');
 canvas.handlers.pointerdown(point(0));
 await [...h.timers.values()][0]();
 assert.deepEqual(acked,[1,2]);
 assert.deepEqual(h.painted,[1]);
 canvas.handlers.pointerup(point(4));
 assert.deepEqual(h.painted,[1,2]);
 await h.click('#copy-selection');
 assert.equal(h.el('#history-output').value,'Tauri');
 assert.equal(h.el('#history-output').selected,true);
 assert.match(h.el('#history-label').textContent,/snapshot 1/);
});
test('application tracking owns the pointer while Shift keeps the same canvas selectable',async()=>{
 const frame=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 frame.session='pty-demo';
 frame.mouse={tracking:'drag',generation:2};
 let allowed=true;
 const forwarded=[];
 const h=harness(async(name,args)=>{
  if(name==='open_shell')return 1;
  if(name==='poll_shell'&&allowed){allowed=false;return frame;}
  if(name==='mouse_shell')forwarded.push(args.report.kind);
 });
 const canvas=h.el('canvas');
 let captured=null;
 canvas.setPointerCapture=id=>captured=id;
 canvas.hasPointerCapture=id=>captured===id;
 canvas.releasePointerCapture=()=>captured=null;
 canvas.getBoundingClientRect=()=>({left:0,top:0,width:frame.cols*10,height:frame.rows.length*10});
 const point=(column,shiftKey)=>({isPrimary:true,pointerType:'mouse',pointerId:1,button:0,shiftKey,
  clientX:column*10+5,clientY:5,preventDefault(){this.defaultPrevented=true;}});
 await h.click('#open');
 canvas.handlers.pointerdown(point(0,false));
 canvas.handlers.pointermove(point(2,false));
 canvas.handlers.pointerup(point(4,false));
 await tick();
 assert.deepEqual(forwarded,['press','motion','release']);
 canvas.handlers.pointerdown(point(0,true));
 canvas.handlers.pointerup(point(4,true));
 await h.click('#copy-selection');
 assert.equal(h.el('#history-output').value,'Tauri');
 assert.deepEqual(forwarded,['press','motion','release']);
});
test('late failed poll from old epoch cannot stop new session',async()=>{
 let epoch=0;const old=deferred();const h=harness(async(name,args)=>{
 if(name==='open_shell')return ++epoch;if(name==='close_shell')return;if(name==='poll_shell')return args.epoch===1?old.promise:null;throw Error(name);
 });const first=h.click('#open');await tick();await h.click('#close');await h.click('#open');old.reject(Error('old session closed'));await first;
 assert.equal(h.timers.size,1);assert.equal(h.el('#input').disabled,false);
});
test('partial send failure locks input to prevent prefix replay',async()=>{
 let writes=0;const h=harness(async name=>{if(name==='open_shell')return 1;if(name==='poll_shell')return null;if(name==='write_shell'){if(++writes===1)return 2;throw Error('write failed');}});
 await h.click('#open');h.el('#input').value='printf hello';await h.submit();assert.equal(h.el('#input').disabled,true);assert.equal(h.el('#send').disabled,true);assert.match(h.el('#status').textContent,/2 bytes accepted/);
});
test('late demo result cannot overwrite a newly opened live session',async()=>{
 const pending=deferred();const fixture=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const h=harness(async name=>{if(name==='demo_frame')return pending.promise;if(name==='open_shell')return 1;if(name==='poll_shell')return null;throw Error(name)});
 const demo=h.click('#demo');assert.equal(h.el('#open').disabled,true);await h.click('#open');pending.resolve(fixture);await demo;
 assert.match(h.el('#status').textContent,/Experimental/);assert.equal(h.el('#demo').disabled,true);
});

test('exit status disables input but keeps tail draining active',async()=>{
 const h=harness(async name=>name==='open_shell'?1:null,args=>({epoch:args.epoch,exited:true,unsupported:'5'}));
 await h.click('#open');assert.equal(h.el('#input').disabled,true);assert.equal(h.timers.size,1);assert.match(h.el('#health').textContent,/Unsupported sequences: 5/);
});
test('stale status cannot disable the next session',async()=>{
 let epoch=0;const old=deferred();
 const h=harness(async name=>name==='open_shell'?++epoch:null,args=>args.epoch===1?old.promise:{epoch:args.epoch,exited:false,unsupported:'0'});
 const first=h.click('#open');await tick();await h.click('#close');await h.click('#open');old.resolve({epoch:1,exited:true,unsupported:'9'});await first;
 assert.equal(h.el('#input').disabled,false);assert.match(h.el('#health').textContent,/Unsupported sequences: 0/);
});

test('late resize failure cannot replace the next session status',async()=>{
 let epoch=0;const old=deferred();
 const h=harness(async name=>{if(name==='open_shell')return ++epoch;if(name==='resize_shell')return old.promise;return null;});
 await h.click('#open');const resize=h.click('#resize');await h.click('#close');await h.click('#open');old.reject(Error('old resize failure'));await resize;
 assert.match(h.el('#status').textContent,/Experimental/);assert.equal(h.el('#resize').disabled,false);
});

test('writer error is surfaced without stopping output draining',async()=>{
 const h=harness(async name=>name==='open_shell'?1:null,args=>({epoch:args.epoch,exited:false,unsupported:'0',write_error:'broken pipe',dropped_reply_bytes:'6'}));
 await h.click('#open');assert.equal(h.el('#input').disabled,true);assert.equal(h.timers.size,1);assert.match(h.el('#health').textContent,/Dropped reply bytes: 6.*broken pipe/);
});
test('only fully drained exited session stops polling',async()=>{
 const h=harness(async name=>name==='open_shell'?1:null,args=>({epoch:args.epoch,exited:true,drained:true,unsupported:'0'}));
 await h.click('#open');assert.equal(h.timers.size,0);assert.equal(h.el('#input').disabled,true);
});

test('failed close retains epoch so cleanup can be retried',async()=>{
 let closes=0;const seen=[];const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='close_shell'){seen.push(args.epoch);if(++closes===1)throw Error('retry');}return null;});
 await h.click('#open');await h.click('#close');assert.equal(h.el('#close').disabled,false);assert.equal(h.el('#open').disabled,true);await h.click('#close');assert.deepEqual(seen,[1,1]);assert.equal(h.el('#open').disabled,false);
});

test('logical key events are serialized and never ANSI encoded in JS',async()=>{
 const seen=[];const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='key_shell')seen.push(args);return null;});await h.click('#open');
 for(const key of ['ArrowUp','a'])h.el('canvas').handlers.keydown({key,shiftKey:false,altKey:false,ctrlKey:false,preventDefault(){}});
 await tick();assert.deepEqual(seen.map(x=>x.name),['ArrowUp','a']);assert.equal(seen[0].epoch,1);
});
test('composition and browser paste shortcut are not double sent',async()=>{
 const seen=[];const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='key_shell'||name==='paste_shell')seen.push([name,args]);return null;});await h.click('#open');
 h.el('canvas').handlers.keydown({key:'a',isComposing:true});h.el('canvas').handlers.keydown({key:'v',ctrlKey:true});
 h.el('canvas').handlers.paste({clipboardData:{getData:()=> 'hello'},preventDefault(){}});await tick();assert.equal(seen.length,1);assert.equal(seen[0][0],'paste_shell');assert.equal(seen[0][1].text,'hello');
});
test('a stuck old keyboard queue does not block a new session',async()=>{
 let epoch=0;const old=deferred(),seen=[];const h=harness(async(name,args)=>{if(name==='open_shell')return ++epoch;if(name==='key_shell'){seen.push(args.epoch);if(args.epoch===1)return old.promise;}return null;});
 const key=()=>h.el('canvas').handlers.keydown({key:'a',preventDefault(){}});
 await h.click('#open');key();await tick();await h.click('#close');await h.click('#open');key();await tick();assert.deepEqual(seen,[1,2]);old.resolve();await tick();assert.equal(h.el('#input').disabled,false);
});

test('line submit waits for previously accepted keys without dropping them',async()=>{
 const first=deferred(),seen=[];let keys=0;
 const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='key_shell'){seen.push(args.name);if(++keys===1)return first.promise;}if(name==='write_shell'){seen.push('command');return args.bytes.length;}return null;});await h.click('#open');
 const key=name=>h.el('canvas').handlers.keydown({key:name,preventDefault(){}});key('a');await tick();key('b');h.el('#input').value='echo';const submit=h.submit();await tick();assert.deepEqual(seen,['a']);first.resolve();await submit;assert.deepEqual(seen,['a','b','command']);
});
test('composition commit reaches text IPC once and stale close commit is dropped',async()=>{
 const seen=[];const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='text_shell')seen.push(args.text);return null;});await h.click('#open');
 const ime=h.el('#ime-input');ime.handlers.compositionstart();ime.handlers.input({data:'zhong',isComposing:true});ime.handlers.compositionend({data:'中文'});ime.handlers.input({data:'中文',inputType:'insertText'});await tick();assert.deepEqual(seen,['中文']);
 ime.handlers.compositionstart();await h.click('#close');ime.handlers.compositionend({data:'旧文本'});ime.handlers.input({data:'旧文本',inputType:'insertText'});await tick();assert.deepEqual(seen,['中文']);
});

test('explicit recovery replaces lost frame and stale poll cannot overwrite it',async()=>{
 const old=deferred();let calls=0;const frame=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));frame.session='pty-demo';frame.revision=8;
 const h=harness(async(name)=>{if(name==='open_shell')return 1;if(name==='poll_shell')return ++calls===1?old.promise:null;if(name==='recover_shell')return frame;if(name==='ack_shell')return;throw Error(name)});
 const opening=h.click('#open');await tick();await h.click('#recover');old.reject(Error('obsolete transport failure'));await opening;
 assert.equal(h.timers.size,1);assert.match(h.el('#status').textContent,/snapshot recovered/);assert.equal(h.el('#input').disabled,false);
});
test('recovery never unlocks input after a partial command write failure',async()=>{
 const frame=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));frame.session='pty-demo';
 const h=harness(async(name)=>{if(name==='open_shell')return 1;if(name==='poll_shell')return null;if(name==='write_shell')throw Error('failed');if(name==='recover_shell')return frame;});
 await h.click('#open');h.el('#input').value='echo hi';await h.submit();await h.click('#recover');assert.equal(h.el('#input').disabled,true);assert.equal(h.el('#send').disabled,true);assert.equal(h.timers.size,1);
});
test('close distinguishes queued cleanup from confirmed direct-child reaping',async()=>{
 for(const pending of [true,false]) {
  const h=harness(async name=>{if(name==='open_shell')return 1;if(name==='close_shell')return {cleanup_pending:pending};return null;});
  await h.click('#open');
  assert.match(h.el('#health').textContent,/Shell running/);
  await h.click('#close');
  assert.equal(h.el('#health').textContent,'');
  assert.match(h.el('#status').textContent,pending?/may still be pending/:/direct child reaped/);
  assert.match(h.el('#status').textContent,/Descendant containment is not provided/);
 }
});
test('visible Unicode snapshot omits wide continuation padding',async()=>{
 const frame=JSON.parse(fs.readFileSync(new URL('../web/unicode-frame.json',import.meta.url),'utf8'));frame.session='pty-demo';let sent=false;
 const h=harness(async name=>{if(name==='open_shell')return 1;if(name==='poll_shell'){if(sent)return null;sent=true;return frame;}return null;});
 await h.click('#open');await h.click('#copy-visible');assert.match(h.el('#history-output').value,/^中文 A/);
});

test('multiline clipboard sends only reviewed text and close invalidates pending paste',async()=>{
 let epoch=0;const sent=[];const h=harness(async(name,args)=>{if(name==='open_shell')return ++epoch;if(name==='paste_shell')sent.push(args);return null;});
 await h.click('#open');const paste=text=>h.el('canvas').handlers.paste({clipboardData:{getData:()=>text},preventDefault(){}});
 paste('echo one\necho two');await tick();assert.equal(sent.length,0);assert.equal(h.el('#paste-review').hidden,false);
 await h.click('#paste-confirm');await tick();assert.equal(sent[0].text,'echo one\necho two');assert.equal(sent[0].epoch,1);
 paste('stale\n');await h.click('#close');await h.click('#open');await h.click('#paste-confirm');await tick();assert.equal(sent.length,1);assert.equal(h.el('#paste-preview').value,'');
});
test('terminal exit discards pending paste without transmitting clipboard',async()=>{
 let exited=false;const sent=[];const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='paste_shell')sent.push(args);return null;},args=>({epoch:args.epoch,exited,unsupported:'0'}));
 await h.click('#open');h.el('#ime-input').handlers.paste({clipboardData:{getData:()=> 'echo unsafe\n'},preventDefault(){}});
 exited=true;await [...h.timers.values()][0]();await h.click('#paste-confirm');await tick();assert.deepEqual(sent,[]);assert.equal(h.el('#paste-review').hidden,true);
});

test('exit during a pending completed line write never unlocks terminal input',async()=>{
 const write=deferred();let exited=false;
 const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='write_shell')return write.promise;return null;},args=>({epoch:args.epoch,exited,unsupported:'0'}));
 await h.click('#open');h.el('#input').value='echo hi';const submit=h.submit();await tick();
 exited=true;await [...h.timers.values()][0]();write.resolve(8);await submit;
 assert.equal(h.el('#input').disabled,true);assert.equal(h.el('#send').disabled,true);
});

test('writer health failure stops a partial line before any suffix is replayed',async()=>{
 const write=deferred(),writes=[];let error=null;
 const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='write_shell'){writes.push(args.bytes);return write.promise;}return null;},args=>({epoch:args.epoch,exited:false,unsupported:'0',write_error:error}));
 await h.click('#open');h.el('#input').value='echo hi';const submit=h.submit();await tick();
 error='broken pipe';await [...h.timers.values()][0]();write.resolve(2);await submit;
 assert.equal(writes.length,1);assert.equal(h.el('#input').disabled,true);
 assert.match(h.el('#status').textContent,/2 bytes accepted.*input became unavailable/);
});

test('poll failure during line write keeps its safety lock after successful completion',async()=>{
 const write=deferred();let fail=false;
 const h=harness(async(name)=>{if(name==='open_shell')return 1;if(name==='write_shell')return write.promise;if(name==='poll_shell'&&fail)throw Error('transport lost');return null;});
 await h.click('#open');h.el('#input').value='ok';const submit=h.submit();await tick();
 fail=true;await [...h.timers.values()][0]();write.resolve(3);await submit;
 assert.equal(h.el('#input').disabled,true);assert.equal(h.el('#send').disabled,true);
 assert.match(h.el('#status').textContent,/transport lost/);
});

test('recovery burst admits one full-frame request while its IPC is pending',async()=>{
 const frame=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));frame.session='pty-demo';
 const pending=deferred();let requests=0;
 const h=harness(async name=>{if(name==='open_shell')return 1;if(name==='recover_shell'){requests++;return pending.promise;}return null;});
 await h.click('#open');const first=h.click('#recover');
 const extras=[];for(let i=0;i<200;i++)extras.push(h.click('#recover'));
 pending.resolve(frame);await Promise.all([first,...extras]);assert.equal(requests,1);
 assert.equal(h.timers.size,1);assert.match(h.el('#status').textContent,/snapshot recovered/);
});

test('failed recovery permits an explicit retry, not an automatic replay',async()=>{
 const frame=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));frame.session='pty-demo';
 let requests=0;
 const h=harness(async name=>{if(name==='open_shell')return 1;if(name==='recover_shell'){if(++requests===1)throw Error('lost');return frame;}return null;});
 await h.click('#open');await h.click('#recover');await tick();
 assert.equal(requests,1);assert.equal(h.timers.size,0);assert.match(h.el('#status').textContent,/Recovery failed/);
 await h.click('#recover');assert.equal(requests,2);assert.equal(h.timers.size,1);
});

test('stuck old recovery neither blocks new epoch nor releases its in-flight guard',async()=>{
 const pending=[deferred(),deferred()];let epoch=0,requests=0;
 const h=harness(async name=>{if(name==='open_shell')return ++epoch;if(name==='recover_shell')return pending[requests++].promise;return null;});
 await h.click('#open');const old=h.click('#recover');await h.click('#close');await h.click('#open');const fresh=h.click('#recover');
 assert.equal(requests,2);pending[0].reject(Error('old'));await old;await h.click('#recover');assert.equal(requests,2);
 const frame=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));frame.session='pty-demo';frame.epoch=2;
 pending[1].resolve(frame);await fresh;assert.equal(h.timers.size,1);assert.match(h.el('#status').textContent,/snapshot recovered/);
});

test('oversized committed text is rejected before encoding without locking input',async()=>{
 const lengths=[],seen=[];
 class BoundedEncoder extends TextEncoder{encode(text){lengths.push(text.length);assert.ok(text.length<=4096);return super.encode(text);}}
 const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='text_shell')seen.push(args.text);return null;},undefined,BoundedEncoder);
 await h.click('#open');const ime=h.el('#ime-input');
 for(const text of ['x'.repeat(1_000_000),'😀'.repeat(2049)]){
  ime.handlers.compositionstart();ime.handlers.compositionend({data:text});await tick();
  assert.deepEqual(lengths,[]);assert.deepEqual(seen,[]);assert.equal(h.el('#input').disabled,false);
  assert.match(h.el('#status').textContent,/4096-byte limit/);
 }
 for(const text of ['é'.repeat(2048),'😀'.repeat(1024)]){
  ime.handlers.compositionstart();ime.handlers.compositionend({data:text});await tick();
  assert.equal(seen.at(-1),text);
 }
 ime.handlers.compositionstart();ime.handlers.compositionend({data:'é'.repeat(2048)+'x'});await tick();
 assert.equal(seen.length,2);assert.equal(h.el('#input').disabled,false);
});
test('command byte bound reserves newline before encoding and permits correction',async()=>{
 const lengths=[],writes=[];
 class BoundedEncoder extends TextEncoder{encode(text){lengths.push(text.length);assert.ok(text.length<=4096);return super.encode(text);}}
 const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='write_shell'){writes.push(args.bytes);return args.bytes.length;}return null;},undefined,BoundedEncoder);
 await h.click('#open');
 for(const text of ['x'.repeat(1_000_000),'x'.repeat(4096)]){
  h.el('#input').value=text;await h.submit();assert.deepEqual(lengths,[]);assert.deepEqual(writes,[]);
  assert.equal(h.el('#input').value,text);assert.equal(h.el('#input').disabled,false);assert.equal(h.el('#send').disabled,false);
 }
 for(const text of ['x'.repeat(4095),'é'.repeat(2047)+'x','😀'.repeat(1023)+'xxx']){
  h.el('#input').value=text;await h.submit();
  assert.equal(writes.at(-1).length,4096);assert.equal(new TextDecoder().decode(Uint8Array.from(writes.at(-1))),text+'\n');
 }
 h.el('#input').value='é'.repeat(2048);await h.submit();assert.equal(writes.length,3);assert.equal(h.el('#input').disabled,false);
});

test('old completed command cannot erase a reopened session draft',async()=>{
 let epoch=0;const old=deferred(),writes=[];
 const h=harness(async(name,args)=>{if(name==='open_shell')return ++epoch;if(name==='write_shell'){writes.push(args);return old.promise;}return null;});
 await h.click('#open');h.el('#input').value='old';const submit=h.submit();await tick();
 await h.click('#close');await h.click('#open');h.el('#input').value='new unsent draft';
 old.resolve(writes[0].bytes.length);await submit;
 assert.equal(h.el('#input').value,'new unsent draft');assert.equal(h.el('#input').disabled,false);
 assert.equal(writes.length,1);assert.equal(writes[0].epoch,1);assert.match(h.el('#status').textContent,/Experimental/);
});

test('stale partial command acknowledgement sends no suffix into reopened session',async()=>{
 let epoch=0;const old=deferred(),writes=[];
 const h=harness(async(name,args)=>{if(name==='open_shell')return ++epoch;if(name==='write_shell'){writes.push(args);return old.promise;}return null;});
 await h.click('#open');h.el('#input').value='old command';const submit=h.submit();await tick();
 await h.click('#close');await h.click('#open');h.el('#input').value='fresh';old.resolve(1);await submit;
 assert.equal(writes.length,1);assert.equal(h.el('#input').value,'fresh');assert.equal(h.el('#input').disabled,false);
});
test('stale command rejection leaves reopened draft and input unlocked',async()=>{
 let epoch=0;const old=deferred();
 const h=harness(async name=>{if(name==='open_shell')return ++epoch;if(name==='write_shell')return old.promise;return null;});
 await h.click('#open');h.el('#input').value='old';const submit=h.submit();await tick();
 await h.click('#close');await h.click('#open');h.el('#input').value='fresh';old.reject(Error('closed old writer'));await submit;
 assert.equal(h.el('#input').value,'fresh');assert.equal(h.el('#input').disabled,false);assert.match(h.el('#status').textContent,/Experimental/);
});
test('current command success still clears draft after exact accepted suffixes',async()=>{
 const writes=[];const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='write_shell'){writes.push(Array.from(args.bytes));return Math.min(2,args.bytes.length);}return null;});
 await h.click('#open');h.el('#input').value='abc';await h.submit();
 assert.deepEqual(writes,[[97,98,99,10],[99,10]]);assert.equal(h.el('#input').value,'');assert.equal(h.el('#input').disabled,false);
});

test('Shift+Insert leaves browser paste available and multiline text still needs review',async()=>{
 const seen=[];let prevented=0;
 const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='key_shell'||name==='paste_shell')seen.push([name,args]);return null;});
 await h.click('#open');
 for(const selector of ['canvas','#ime-input']){
  h.el(selector).handlers.keydown({key:'Insert',shiftKey:true,ctrlKey:false,altKey:false,preventDefault(){prevented++;}});
  await tick();assert.equal(prevented,0);assert.deepEqual(seen,[]);
  h.el(selector).handlers.paste({clipboardData:{getData:()=> 'echo first\necho second'},preventDefault(){}});
  await tick();assert.deepEqual(seen,[]);assert.equal(h.el('#paste-review').hidden,false);
  await h.click('#paste-cancel');
 }
 h.el('canvas').handlers.paste({clipboardData:{getData:()=> 'echo first\necho second'},preventDefault(){}});
 await h.click('#paste-confirm');await tick();
 assert.equal(seen.length,1);assert.equal(seen[0][0],'paste_shell');assert.equal(seen[0][1].text,'echo first\necho second');
});

test('plain Insert and modified non-paste Insert keep terminal key semantics',async()=>{
 const seen=[];let prevented=0;
 const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='key_shell')seen.push(args);return null;});await h.click('#open');
 for(const modifiers of [{shiftKey:false,ctrlKey:false,altKey:false},{shiftKey:true,ctrlKey:true,altKey:false},{shiftKey:true,ctrlKey:false,altKey:true}]){
  h.el('canvas').handlers.keydown({key:'Insert',...modifiers,preventDefault(){prevented++;}});
 }
 await tick();assert.equal(prevented,3);assert.equal(seen.length,3);assert.deepEqual(seen.map(x=>[x.name,x.shift,x.ctrl,x.alt]),[['Insert',false,false,false],['Insert',true,true,false],['Insert',true,false,true]]);
});

test('paste review dismissal returns focus through the production paste event origin',async()=>{
 const sent=[];const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='paste_shell')sent.push(args.text);return null;});await h.click('#open');
 const panel=h.el('#paste-review'),document={activeElement:null};panel.ownerDocument=document;panel.contains=target=>target===h.el('#paste-confirm')||target===h.el('#paste-cancel');
 for(const selector of ['canvas','#ime-input']){
  let focused=0;const origin=h.el(selector);origin.isConnected=true;origin.focus=()=>{focused++;document.activeElement=origin;};
  origin.handlers.paste({currentTarget:origin,clipboardData:{getData:()=> 'review\n'},preventDefault(){}});
  document.activeElement=h.el('#paste-confirm');await h.click('#paste-confirm');await tick();assert.equal(focused,1);assert.equal(document.activeElement,origin);
 }
 assert.deepEqual(sent,['review\n','review\n']);
});
test('review Escape at canvas or IME origin cancels rather than reaching shell',async()=>{
 for(const selector of ['canvas','#ime-input']){
  const seen=[];const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='key_shell'||name==='paste_shell')seen.push([name,args]);return null;});await h.click('#open');
  const origin=h.el(selector),panel=h.el('#paste-review');panel.ownerDocument={activeElement:origin};
  origin.handlers.paste({currentTarget:origin,clipboardData:{getData:()=> 'echo a\necho b'},preventDefault(){}});
  let prevented=0;origin.handlers.keydown({key:'Escape',target:origin,currentTarget:origin,preventDefault(){prevented++;},stopPropagation(){}});
  await tick();assert.equal(panel.hidden,true);assert.equal(prevented,1);assert.deepEqual(seen,[]);
  h.click('#paste-confirm');await tick();assert.deepEqual(seen,[]);
  origin.handlers.keydown({key:'Escape',target:origin,currentTarget:origin,preventDefault(){}});
  await tick();assert.equal(seen.length,1);assert.equal(seen[0][0],'key_shell');assert.equal(seen[0][1].name,'Escape');
 }
});
test('already consumed terminal keys never reach shell or dismiss paste review',async()=>{
 for(const selector of ['canvas','#ime-input']){
  const sent=[];const h=harness(async(name,args)=>{if(name==='open_shell')return 1;if(name==='key_shell')sent.push(args);return null;});await h.click('#open');
  const origin=h.el(selector),panel=h.el('#paste-review');panel.ownerDocument={activeElement:origin};
  origin.handlers.paste({currentTarget:origin,clipboardData:{getData:()=> 'review\n'},preventDefault(){}});
  for(const key of ['Escape','Enter','ArrowUp','c'])origin.handlers.keydown({key,ctrlKey:key==='c',defaultPrevented:true,currentTarget:origin,target:origin,preventDefault(){},stopPropagation(){}});
  await tick();assert.deepEqual(sent,[]);assert.equal(panel.hidden,false);
  origin.handlers.keydown({key:'ArrowUp',currentTarget:origin,target:origin,preventDefault(){}});await tick();assert.equal(sent.length,1);
 }
});
