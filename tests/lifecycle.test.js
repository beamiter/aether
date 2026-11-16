import {installPasteReview} from '../web/paste.js';
import {resizeQueue,cursorTarget} from '../web/viewport.js';
import test from 'node:test';import assert from 'node:assert/strict';import fs from 'node:fs';import vm from 'node:vm';
import {installHistory} from '../web/history.js';
import {acceptFull} from '../web/frames.js'; import {installIme} from '../web/ime.js';
const tick=()=>new Promise(r=>setImmediate(r));
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return {promise,resolve,reject};};
function harness(rawInvoke, health=args=>({epoch:args.epoch,exited:false,unsupported:"0"}), Encoder=TextEncoder){
 const invoke=(name,args)=>name==='shell_status'?Promise.resolve(health(args)).then(s=>({drained:false,write_error:null,dropped_reply_bytes:'0',...s})):rawInvoke(name,args);
 const elements=new Map(),timers=new Map();let id=0;
 const el=selector=>{if(!elements.has(selector))elements.set(selector,{disabled:false,value:'',textContent:'',style:{},handlers:{},focus(){},addEventListener(name,fn){this.handlers[name]=fn}});return elements.get(selector)};
 const context={document:{querySelector:el},__TAURI__:{core:{invoke}},acceptFull,installIme,installHistory,installPasteReview,resizeQueue,cursorTarget,paint(){},TextEncoder:Encoder,TextDecoder,
 setInterval(fn){timers.set(++id,fn);return id},clearInterval(i){timers.delete(i)},setTimeout};
 vm.runInNewContext(fs.readFileSync(new URL('../web/app.js',import.meta.url),'utf8').replace(/^import .*\n/gm,''),context);
 return {el,timers,click:s=>el(s).handlers.click(),submit:()=>el('#command').handlers.submit({preventDefault(){}})};
}
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
  await h.click('#open');await h.click('#close');
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
