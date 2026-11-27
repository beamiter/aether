import {directionalPaneNeighbor,retainedPaneTree} from '../web/pane-geometry.js';
import {makePassiveSurfaces} from '../web/passive-surfaces.js';
import {validateReloadView,retainedReloadView,saveReloadTicket,takeReloadTicket} from '../web/ssh-reload.js';
import {installPasteReview} from '../web/paste.js';
import {installTerminalFocus} from '../web/terminal-focus.js';
import {installTerminalMouse} from '../web/terminal-mouse.js';
import {installTerminalSelection} from '../web/terminal-selection.js';
import {installTerminalShell} from '../web/terminal-shell.js';
import {dimensions,resizeQueue,cursorTarget,terminalCellAt,viewportDimensions,windowPixelQueue} from '../web/viewport.js';
import test from 'node:test';import assert from 'node:assert/strict';import fs from 'node:fs';import vm from 'node:vm';
import {installHistory} from '../web/history.js';
import {installInlineHistory} from '../web/inline-history.js';
import {installCommandBlocks} from '../web/command-blocks.js';
import {installTerminalChat,validateChatStatus} from '../web/terminal-chat.js';
import {installWorkflows} from '../web/workflows.js';
import {plainLocalTab,readLocalShellLayout,saveLocalShellLayout,readThreePaneLayout,saveThreePaneLayout,MAX_LOCAL_SHELLS,validLocalGeometry,localSplitPair} from '../web/local-layout.js';
import {acceptFull,acceptUpdate,backingGeometry,clearPaint,paint as paintFrame} from '../web/frames.js'; import {installIme} from '../web/ime.js';
import {acceptChannelFrame,channelCounter,CHANNEL_RENDITION_VERSION} from '../web/frames-v4.js';
import {packedRow} from './fixtures/packed-channel.js';
const tick=()=>new Promise(r=>setImmediate(r));
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return {promise,resolve,reject};};
function harness(rawInvoke, health=args=>({epoch:args.epoch,exited:false,unsupported:"0"}), Encoder=TextEncoder, paint=()=>{},storage={},Channel,animation={}){
 const invoke=async(name,args)=>{
  if(name==='set_shell_window_pixels')return animation.applyWindowPixels?.(args)??null;
  if(name==='ssh_terminal'&&args?.action?.operation==='recover_reload_layout')return animation.recoverReloadLayout?.()??null;
  const {owner,...parameters}=args??{};
  const keepsOwner=['activate_local_tab','subscribe_local_shell','ack_local_stream','set_local_observer','set_local_observers','observe_local_shell','local_command_blocks','focus_shell'].includes(name);
  const legacy=owner&&!keepsOwner?{epoch:owner.epoch,...parameters}:args;
  if(name==='shell_status')return {drained:false,write_error:null,dropped_reply_bytes:'0',...await health(legacy)};
  const result=await rawInvoke(name,legacy);
  if(name==='open_shell'&&Number.isInteger(result)){
   return {session:result.toString(16).padStart(32,'0'),epoch:result,generation:1};
  }
  if(name==='poll_shell'&&result&&!result.kind)return {kind:'snapshot',frame:{...result,session:owner.session}};
  if(name==='recover_shell'&&result&&!result.owner){
   return {owner:{...owner,generation:owner.generation+1},frame:{...result,session:owner.session}};
  }
  return result;
 };
 const elements=new Map(),timers=new Map();let id=0;
 const element=()=>({children:[],disabled:false,value:'',textContent:'',style:{},dataset:{},handlers:{},classList:{toggle(){}},focus(){},append(child){this.children.push(child);},showModal(){this.open=true;},close(){this.open=false;},click(){return this.handlers.click?.();},getAttribute(name){return this.attributes.get(name)??null;},scrollTo(){},attributes:new Map(),setAttribute(name,value){this.attributes.set(name,String(value));},removeAttribute(name){this.attributes.delete(name);},replaceChildren(...options){this.options=options;},addEventListener(name,fn){this.handlers[name]=fn}});
 const el=selector=>{if(!elements.has(selector))elements.set(selector,element());return elements.get(selector)};
 const canvasEvents=new Map();
 if (animation.mouseCanvas) {
  const canvas=el('canvas'), captured=new Set();
  canvas.setPointerCapture=id=>captured.add(id);
  canvas.releasePointerCapture=id=>captured.delete(id);
  canvas.hasPointerCapture=id=>captured.has(id);
  canvas.getBoundingClientRect=()=>({left:0,top:0,width:600,height:300});
  canvas.addEventListener=(name,fn,options)=>{
   canvas.handlers[name]=fn;
   const listeners=canvasEvents.get(name)??[];
   listeners.push({fn,capture:options===true||options?.capture===true});
   canvasEvents.set(name,listeners);
  };
 }
 const pageEvents=new Map(),windowEvents=new Map();
 const listen=events=>(name,fn)=>events.set(name,[...(events.get(name)??[]),fn]);
 const fire=(events,name)=>{for(const fn of events.get(name)??[])fn({target:document.activeElement});};
 const document={querySelector:el,querySelectorAll:()=>[],createElement:element,body:element(),activeElement:null,hidden:false,hasFocus:()=>true,addEventListener:listen(pageEvents)};
 const context={directionalPaneNeighbor,retainedPaneTree,makePassiveSurfaces,validateReloadView,retainedReloadView,saveReloadTicket,takeReloadTicket,installFilePreviewPanel:()=>({dismiss(){}}),document,addEventListener:listen(windowEvents),__TAURI__:{core:{invoke,Channel}},acceptFull,acceptUpdate,backingGeometry,clearPaint,acceptChannelFrame,channelCounter,CHANNEL_RENDITION_VERSION,installIme,installHistory,installCommandBlocks,installTerminalChat,installTerminalShell,installWorkflows,validateChatStatus,installPasteReview,installTerminalFocus,installTerminalMouse,installTerminalSelection,dimensions,resizeQueue,cursorTarget,terminalCellAt,viewportDimensions,windowPixelQueue,paint,TextEncoder:Encoder,TextDecoder,
 installInlineHistory,
 plainLocalTab,MAX_LOCAL_SHELLS,validLocalGeometry,localSplitPair,readLocalShellLayout:()=>readLocalShellLayout(storage),saveLocalShellLayout:value=>saveLocalShellLayout(value,storage),readThreePaneLayout:()=>readThreePaneLayout(storage),saveThreePaneLayout:value=>saveThreePaneLayout(value,storage),
 setInterval(fn){timers.set(++id,fn);return id},clearInterval(i){timers.delete(i)},setTimeout,...animation};
 vm.runInNewContext(fs.readFileSync(new URL('../web/app.js',import.meta.url),'utf8').replace(/^import .*\n/gm,''),context);
 return {el,timers,keydown(event){for(const fn of pageEvents.get('keydown')??[])fn(event);},click:s=>el(s).handlers.click(),submit:()=>el('#command').handlers.submit({preventDefault(){}}),submitSsh:()=>el('#ssh-terminal-form').handlers.submit({preventDefault(){}}),
  focus(selector){document.activeElement=el(selector);fire(pageEvents,'focusout');fire(pageEvents,'focusin');},
  visibility(hidden){document.hidden=hidden;fire(pageEvents,'visibilitychange');},
  pointer(name,extra={}){
   const event={isPrimary:true,pointerType:'mouse',pointerId:1,clientX:1,clientY:1,button:0,
    preventDefault(){this.defaultPrevented=true;},...extra};
   const listeners=canvasEvents.get(name)??[];
   for(const capture of [true,false])for(const entry of listeners)if(entry.capture===capture)entry.fn(event);
  },
  windowFocus(focused){fire(windowEvents,focused?'focus':'blur');}};
}

const channelHealth=epoch=>({epoch,exited:false,drained:false,write_error:null,dropped_reply_bytes:'0',unsupported:'0'});

test('history Escape returns to the opening pane except during composition or after owner changes',async()=>{
 let epoch=0;
 const h=harness(async(name)=>name==='open_shell'?++epoch:null);
 const panel=h.el('#terminal-history'),ime=h.el('#ime-input');
 panel.querySelector=()=>h.el('#history-find-query');
 panel.scrollIntoView=()=>{};
 h.el('#history-refresh').click=()=>{};
 let focused=0;
 ime.focus=()=>{focused++;};
 function escape(extra={}){
  const event={key:'Escape',preventDefault(){this.defaultPrevented=true;},...extra};
  panel.handlers.keydown(event);
  return event.defaultPrevented;
 }
 await h.click('#open');await tick();
 h.click('#show-history');
 const before=focused;
 assert.equal(escape({isComposing:true}),undefined);
 assert.equal(panel.open,true);
 assert.equal(focused,before);
 assert.equal(escape(),true);
 assert.equal(panel.open,false);
 assert.equal(focused,before+1);

 h.click('#show-history');
 await h.click('#close');
 await h.click('#open');await tick();
 const newPaneFocus=focused;
 assert.equal(escape(),true);
 assert.equal(panel.open,false);
 assert.equal(focused,newPaneFocus,'the old history surface must not redirect the new pane');
});

test('Find uses the active pane without scrolling and History keeps its own full panel',async()=>{
 const h=harness(async name=>name==='open_shell'?1:null);
 const history=h.el('#terminal-history'),pane=h.el('#active-pane'),dock=h.el('#terminal-history-dock');
 const query=h.el('#history-find-query'),ime=h.el('#ime-input');
 const placement=[],focused=[];
 let pageScrolls=0;
 history.querySelector=()=>query;
 history.scrollIntoView=()=>pageScrolls++;
 h.el('#history-refresh').click=()=>{};
 pane.append=node=>placement.push(['pane',node]);
 dock.append=node=>placement.push(['dock',node]);
 query.focus=options=>focused.push(['query',options?.preventScroll]);
 ime.focus=options=>focused.push(['pty',options?.preventScroll]);

 await h.click('#open');await tick();
 h.el('#history-find-form').hidden=false;
 const find=()=>h.keydown({key:'F',ctrlKey:true,shiftKey:true,target:{closest:()=>true},preventDefault(){}});
 const close=()=>history.handlers.keydown({key:'Escape',preventDefault(){}});
 find();
 assert.equal(history.open,true);
 assert.deepEqual(placement.at(-1),['pane',history]);
 assert.deepEqual(focused.at(-1),['query',true]);
 assert.equal(pageScrolls,0);
 close();
 assert.equal(history.open,false);
 assert.deepEqual(placement.at(-1),['dock',history]);
 assert.deepEqual(focused.at(-1),['pty',true]);
 assert.equal(pageScrolls,0);

 find();
 h.click('#show-history');
 assert.deepEqual(placement.at(-1),['dock',history]);
 assert.equal(history.open,true);
 assert.equal(pageScrolls,1);
 close();

 find();
 await h.click('#close');
 assert.equal(history.open,false,'the old search must close on detach');
 assert.deepEqual(placement.at(-1),['dock',history]);
});

test('auto-fit keeps a return to painted geometry while a smaller resize is in flight',async()=>{
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const owner={session:'a'.repeat(32),epoch:1,generation:1};
 const baseline={...raw,session:owner.session};
 const original=[baseline.rows.length,baseline.cols];
 const smaller=[original[0]-1,original[1]-1];
 const outstanding=deferred(),resizes=[];
 let observeFit;
 const h=harness(async(name,args)=>{
  if(name==='open_shell')return owner;
  if(name==='poll_shell')return baseline;
  if(name==='resize_shell'){
   resizes.push([args.rows,args.cols]);
   if(resizes.length===1)return outstanding.promise;
   return null;
  }
  return null;
 },undefined,undefined,undefined,undefined,undefined,{
  ResizeObserver:class{constructor(fn){observeFit=fn}observe(){}},
  getComputedStyle:()=>({}),
 });
 const viewport=h.el('#active-pane .viewport');
 let [rows,cols]=original;
 viewport.getBoundingClientRect=()=>({height:rows*24,width:cols*12});
 viewport.offsetWidth=viewport.clientWidth=0;
 const fit=h.el('#fit-size');
 fit.checked=false;
 await h.click('#open');await tick();
 fit.checked=true;fit.handlers.change();
 assert.deepEqual(resizes,[],'settled paint should not send redundant IPC');
 [rows,cols]=smaller;observeFit();await tick();
 assert.deepEqual(resizes,[smaller]);
 [rows,cols]=original;observeFit();
 assert.deepEqual(resizes,[smaller],'one IPC at a time');
 outstanding.resolve();await tick();await tick();
 assert.deepEqual(resizes,[smaller,original],'restored viewport must become the final size');
});

test('local terminal focus follows the viewport, IME, window and document visibility',async()=>{
 const shell={session:'a'.repeat(32),epoch:1,generation:1};
 const signals=[];
 const h=harness(async(name,args)=>{
  if(name==='open_shell')return shell;
  if(name==='poll_shell')return null;
  if(name==='focus_shell'){
   signals.push([args.owner.generation,args.focused]);
   if(!args.focused)throw Error('focus report unavailable');
   return true;
  }
  throw Error('unexpected local IPC '+name);
 });
 await h.click('#open');
 h.focus('canvas');await tick();
 h.focus('#ime-input');await tick();
 h.windowFocus(false);h.windowFocus(false);await tick();
 h.windowFocus(true);await tick();
 h.visibility(true);await tick();
 h.visibility(false);await tick();
 h.focus('#input');await tick();
 assert.deepEqual(signals,[[1,true],[1,false],[1,true],[1,false],[1,true],[1,false]]);
 assert.equal(h.el('#input').disabled,false);
});

test('focus queued before detach cannot reach a stale or read-only attachment',async()=>{
 const shell={session:'a'.repeat(32),epoch:1,generation:1};
 const reattached={...shell,generation:3};
 const frame=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const running=deferred(),signals=[];
 const h=harness(async(name,args)=>{
  if(name==='open_shell')return shell;
  if(name==='poll_shell'||name==='detach_shell'||name==='ack_shell')return null;
  if(name==='attach_shell')return {owner:reattached,frame:{...frame,session:shell.session}};
  if(name==='focus_shell'){
   signals.push([args.owner.generation,args.focused]);
   return running.promise;
  }
  throw Error('unexpected local IPC '+name);
 });
 await h.click('#open');
 h.focus('canvas');await tick();
 h.windowFocus(false);await tick();
 await h.click('#detach');
 await h.click('#attach');
 h.windowFocus(true);h.focus('#ime-input');
 running.resolve(true);await tick();
 assert.deepEqual(signals,[[1,true]]);
 assert.equal(h.el('#input').disabled,true);
});

test('SSH focus requires an explicit capability for each writable attachment',async()=>{
 const first={session:'a'.repeat(32),epoch:1,generation:1};
 const recovered={...first,generation:2},readonly={...first,generation:4};
 const frame={...JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8')),session:first.session,epoch:1};
 const signals=[];
 let advertised;
 const h=harness(async(name,{action})=>{
  assert.equal(name,'ssh_terminal');
  if(action.operation==='open')return first;
  if(action.operation==='attach')return {owner:readonly,frame};
  const command=action.command;
  switch(command.kind){
   case 'poll':case 'ack':case 'detach':return null;
   case 'status':return {...channelHealth(command.owner.epoch),focus_reporting:advertised};
   case 'recover':return {owner:recovered,frame,input_allowed:true};
   case 'focus':signals.push([command.owner.generation,command.focused]);return true;
   default:throw Error(command.kind);
  }
 });
 h.el('#ssh-terminal-host').value='lab';await h.submitSsh();
 h.focus('canvas');await tick();
 assert.deepEqual(signals,[]);
 advertised='true';await [...h.timers.values()][0]();await tick();
 assert.deepEqual(signals,[]);
 advertised=true;await [...h.timers.values()][0]();await tick();
 h.windowFocus(false);await tick();
 h.windowFocus(true);await tick();
 assert.deepEqual(signals,[[1,true],[1,false],[1,true]]);
 advertised=undefined;await h.click('#recover');await tick();
 assert.deepEqual(signals,[[1,true],[1,false],[1,true]]);
 advertised=true;await [...h.timers.values()][0]();await tick();
 assert.deepEqual(signals,[[1,true],[1,false],[1,true],[2,true]]);
 await h.click('#detach');await h.click('#attach');
 h.focus('#ime-input');await tick();
 assert.deepEqual(signals,[[1,true],[1,false],[1,true],[2,true]]);
 assert.equal(h.el('#input').disabled,true);
});

test('SSH mouse requires a negotiated writable owner and its matching visible frame',async()=>{
 const first={session:'b'.repeat(32),epoch:1,generation:1},readonly={...first,generation:3};
 const frame={...JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8')),session:first.session,epoch:1};
 const descriptor={layout:String(frame.layout),revision:String(frame.revision),generation:7,tracking:'drag'};
 const reports=[];
 let firstPoll=true,capability,mouse=descriptor;
 const h=harness(async(name,{action})=>{
  assert.equal(name,'ssh_terminal');
  if(action.operation==='open')return first;
  if(action.operation==='attach')return {owner:readonly,frame};
  const command=action.command;
  switch(command.kind){
   case 'poll':if(firstPoll){firstPoll=false;return {kind:'snapshot',frame};}return null;
   case 'ack':case 'detach':return null;
   case 'status':return {...channelHealth(command.owner.epoch),mouse_reporting:capability,mouse};
   case 'mouse':reports.push(command);return true;
   default:throw Error(command.kind);
  }
 },undefined,undefined,()=>{}, {},undefined,{mouseCanvas:true});
 const poll=async()=>{await [...h.timers.values()][0]();await tick();};
 const click=async(extra)=>{h.pointer('pointerdown',extra);h.pointer('pointerup',extra);await tick();};
 h.el('#ssh-terminal-host').value='lab';await h.submitSsh();
 await click();assert.deepEqual(reports,[]);
 capability=true;mouse={...descriptor,revision:String(Number(descriptor.revision)+1)};
 await poll();await click();assert.deepEqual(reports,[]);
 mouse=descriptor;await poll();
 await click({shiftKey:true});assert.deepEqual(reports,[]);
 h.pointer('pointerdown');await tick();
 mouse=undefined;await poll();
 h.pointer('pointerup');await tick();
 assert.deepEqual(reports.map(({owner,report})=>[owner.generation,report.kind,report.layout,report.revision,report.generation,report.row,report.col]),[
  [1,'press',descriptor.layout,descriptor.revision,7,0,0],
  [1,'release',descriptor.layout,descriptor.revision,7,0,0],
 ]);
 await h.click('#detach');await h.click('#attach');
 mouse=descriptor;await poll();await click();
 assert.equal(reports.length,2);
 assert.equal(h.el('#input').disabled,true);
});

test('SSH alternate scroll uses a negotiated visible stamp and defers to mouse tracking',async()=>{
 const owner={session:'d'.repeat(32),epoch:1,generation:1};
 const frame={...JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8')),session:owner.session,epoch:1};
 const stamp={layout:String(frame.layout),revision:String(frame.revision),generation:8};
 const reports=[];
 let firstPoll=true,capability,alternate={...stamp},mouse;
 const h=harness(async(name,{action})=>{
  assert.equal(name,'ssh_terminal');
  if(action.operation==='open')return owner;
  const command=action.command;
  switch(command.kind){
   case 'poll':if(firstPoll){firstPoll=false;return {kind:'snapshot',frame};}return null;
   case 'ack':return null;
   case 'status':return {...channelHealth(command.owner.epoch),mouse_reporting:capability,mouse,alternate_scroll:alternate};
   case 'mouse':reports.push(command);return true;
   default:throw Error(command.kind);
  }
 },undefined,undefined,()=>{}, {},undefined,{mouseCanvas:true});
 const poll=async()=>{await [...h.timers.values()][0]();await tick();};
 const wheel=async()=>{h.pointer('wheel',{deltaX:0,deltaY:-1,deltaMode:1});await tick();};
 h.el('#ssh-terminal-host').value='lab';await h.submitSsh();
 await wheel();assert.deepEqual(reports,[]);
 capability=true;alternate={...stamp,revision:String(Number(stamp.revision)+1)};
 await poll();await wheel();assert.deepEqual(reports,[]);
 alternate=stamp;await poll();
 h.pointer('pointerdown');h.pointer('pointerup');await tick();
 assert.deepEqual(reports,[]);
 await wheel();
 assert.deepEqual(reports.map(({owner,report})=>[owner.generation,report.kind,report.revision,report.generation]),[
  [1,'wheel_up',stamp.revision,8],
 ]);
 mouse={...stamp,generation:9,tracking:'button'};
 await poll();h.pointer('pointerdown');h.pointer('pointerup');await tick();
 assert.deepEqual(reports.map(({report})=>[report.kind,report.generation]),[
  ['wheel_up',8],['press',9],['release',9],
 ]);
});

test('terminal Chat sends each displayed prompt at most once while preserving normal input',async()=>{
 const submissions=[];
 const prompt=value=>({surface:'prompt',prompt:value,newest_completed:null});
 let chat=prompt('1');
 const h=harness(async(name,args)=>{
  if(name==='open_shell')return 1;
  if(name==='poll_shell')return null;
  if(name==='local_command_blocks')return {owner:args.owner,enabled:true,blocks:[],bookmarked:[]};
  if(name==='submit_local_chat'){
   const pending=deferred();submissions.push({prompt:args.prompt,...pending});return pending.promise;
  }
  throw Error('unexpected IPC '+name);
 },({epoch})=>({...channelHealth(epoch),chat}));
 await h.click('#open-integrated');
 h.click('#terminal-chat-toggle');
 h.click('#terminal-chat-send');
 h.click('#terminal-chat-send');
 await tick();
 assert.deepEqual(submissions.map(entry=>entry.prompt),['1']);
 submissions[0].resolve(true);await tick();
 h.click('#terminal-chat-send');await tick();
 assert.equal(submissions.length,1);
 assert.equal(h.el('#terminal-chat-send').disabled,true);
 chat=prompt('2');
 await h.timers.values().next().value();
 h.click('#terminal-chat-send');await tick();
 assert.deepEqual(submissions.map(entry=>entry.prompt),['1','2']);
 submissions[1].resolve(false);await tick();
 assert.equal(h.el('#input').disabled,false);
 assert.match(h.el('#status').textContent,/shell prompt changed/);
});

function channelSnapshot(frame){
 const {cols,rows,...stamp}=frame;
 return JSON.stringify({kind:'snapshot',cols,rows:rows.map(cells=>packedRow({wrapped:false,cells})),
   stamp:{...stamp,version:CHANNEL_RENDITION_VERSION,epoch:String(stamp.epoch),resource_generation:String(stamp.epoch),revision:String(stamp.revision),layout:String(stamp.layout),screen:'primary'}});
}
function testChannel(){
 let number=40;
 return class {constructor(){this.id=++number;}};
}

test('OSC8 hover remains current across a new frame without synthesizing a click',async()=>{
 const owner={session:'a'.repeat(32),epoch:1,generation:1};
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const target='https://aether-hyperlink.test/docs';
 let outgoing,opened=0;
 const h=harness(async(name,args)=>{
  switch(name){
   case 'open_shell':return owner;
   case 'subscribe_local_shell':outgoing=args.stream.onmessage;return null;
   case 'ack_local_stream':return null;
   case 'open_terminal_hyperlink':opened++;return null;
   default:throw Error(name);
  }
 },undefined,undefined,undefined,{},testChannel(),{mouseCanvas:true});
 const canvas=h.el('canvas');
 canvas.getBoundingClientRect=()=>({left:0,top:0,width:raw.cols*12,height:raw.rows.length*24});
 const frame=(sequence,linked)=> {
  const payload=JSON.parse(channelSnapshot({...raw,session:owner.session,epoch:1,revision:sequence}));
  if(linked){
   payload.rows[0]=packedRow({wrapped:false,cells:raw.rows[0].map((cell,index)=>index===0?{...cell,hyperlink_id:17}:cell)});
   payload.hyperlinks=[{id:17,target}];
  }
  return outgoing({owner,kind:'update',sequence:String(sequence),frame:JSON.stringify(payload),status:channelHealth(1)});
 };
 await h.click('#open');await frame(1,true);
 canvas.handlers.pointermove({clientX:2,clientY:2,ctrlKey:true});
 assert.equal(canvas.title,`Ctrl-click to open ${target}`);
 assert.equal(canvas.style.cursor,'pointer');
 await frame(2,true);
 assert.equal(canvas.title,`Ctrl-click to open ${target}`);
 assert.equal(canvas.style.cursor,'pointer');
 await frame(3,false);
 assert.equal(canvas.title,'');
 await frame(4,true);
 assert.equal(canvas.title,`Ctrl-click to open ${target}`);
 canvas.handlers.pointerleave({});
 await frame(5,true);
 assert.equal(canvas.title,'');
 assert.equal(opened,0);
});

test('a block snapshot from an old attachment cannot populate the reconnected panel',async()=>{
 const first={session:'a'.repeat(32),epoch:1,generation:1},next={...first,generation:3};
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const reads=[];
 const h=harness(async(name,args)=>{
  switch(name){
   case 'open_shell':return first;
   case 'poll_shell':case 'detach_shell':case 'ack_shell':return null;
   case 'attach_shell':return {owner:next,frame:{...raw,session:next.session}};
   case 'local_command_blocks':{
    const pending=deferred();reads.push({owner:args.owner,...pending});return pending.promise;
   }
   default:throw Error('unexpected IPC '+name);
  }
 });
 await h.click('#open-integrated');
 h.click('#command-block-toggle');
 await h.click('#detach');
 await h.click('#attach');
 h.click('#command-block-toggle');
 assert.deepEqual(reads.map(read=>read.owner.generation),[1,3]);
 reads[0].resolve({owner:first,enabled:false,blocks:[],bookmarked:[]});await tick();
 assert.equal(h.el('#command-block-status').textContent,'');
 reads[1].resolve({owner:next,enabled:true,blocks:[],bookmarked:[]});await tick();
 assert.match(h.el('#command-block-status').textContent,/Waiting for a completed command/);
 assert.equal(h.el('#input').disabled,true);
});

test('local Channel paints and acknowledges exact revisions, fencing a retired attachment',async()=>{
 const first={session:'a'.repeat(32),epoch:1,generation:1},next={...first,generation:3};
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const frame={...raw,session:first.session};
 const sent=[],receivers=[],painted=[],scheduled=new Map();let animation=0;
 const h=harness(async(name,args)=>{
  switch(name){
   case 'open_shell':return first;
   case 'subscribe_local_shell':assert.equal(args.frameVersion,CHANNEL_RENDITION_VERSION);receivers.push(args.stream.onmessage);return null;
   case 'ack_local_stream':sent.push(args);return null;
   case 'detach_shell':case 'ack_shell':return null;
   case 'attach_shell':assert.equal(args.frameVersion,CHANNEL_RENDITION_VERSION);return {owner:next,frame:channelSnapshot(frame)};
   default:throw Error('unexpected local IPC '+name);
  }
 },undefined,undefined,(_canvas,frame)=>painted.push(frame.revision),{},testChannel(),{
  requestAnimationFrame:fn=>{scheduled.set(++animation,fn);return animation;},
  cancelAnimationFrame:id=>scheduled.delete(id),
 });
 await h.click('#open');
 const revision='9007199254740993';
 await receivers[0]({owner:first,kind:'update',sequence:'1',frame:channelSnapshot({...frame,revision}),status:channelHealth(1)});
 assert.equal(sent.length,0);assert.equal(painted.length,0);
 const draw=scheduled.get(1);scheduled.delete(1);draw();await tick();
 assert.equal(h.el('#health').dataset.transport,'channel');
 assert.deepEqual(sent.map(({owner,streamId,sequence,revision})=>[owner.generation,streamId,sequence,revision]),[[1,41,'1',revision]]);
 await receivers[0]({owner:first,kind:'update',sequence:'2',frame:channelSnapshot({...frame,revision:'9007199254740994'}),status:channelHealth(1)});
 assert.equal(scheduled.size,1);
 await h.click('#detach');assert.equal(scheduled.size,0);await h.click('#attach');
 await receivers[0]({owner:first,kind:'update',sequence:'2',frame:null,status:channelHealth(1)});
 assert.equal(sent.length,1);assert.equal(painted.length,2);
 await receivers[1]({owner:next,kind:'update',sequence:'1',frame:null,status:channelHealth(1)});
 assert.deepEqual(sent.map(({owner,streamId,sequence,revision})=>[owner.generation,streamId,sequence,revision]),[[1,41,'1',revision],[3,42,'1',null]]);
});

test('a rejected stream snapshot requires recovery before compatibility polling can resume',async()=>{
 const first={session:'a'.repeat(32),epoch:1,generation:1},next={...first,generation:2};
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const frame={...raw,session:first.session};
 const calls=[],painted=[];let outgoing;
 const h=harness(async(name,args)=>{
  calls.push(name);
  switch(name){
   case 'open_shell':return first;
   case 'subscribe_local_shell':outgoing=args.stream.onmessage;return null;
   case 'recover_shell':return {owner:next,frame,input_allowed:true};
   case 'ack_shell':case 'poll_shell':return null;
   default:throw Error('unexpected local IPC '+name);
  }
 },undefined,undefined,(_canvas,frame)=>painted.push(frame.revision),{},testChannel());
 await h.click('#open');
 await outgoing({owner:first,kind:'update',sequence:'1',frame:channelSnapshot(frame),status:{}});
 assert.equal(painted.length,0);assert.equal(h.el('#input').disabled,true);
 assert.match(h.el('#status').textContent,/Recover the output snapshot/);
 assert.deepEqual(calls,['open_shell','subscribe_local_shell']);
 await h.click('#recover');
 assert.equal(h.el('#input').disabled,false);assert.equal(h.el('#health').dataset.transport,'poll');
 assert.deepEqual(calls,['open_shell','subscribe_local_shell','recover_shell','ack_shell','poll_shell']);
 assert.deepEqual(painted,[frame.revision]);
});

test('a missing image resource requests a fresh graphics snapshot on the same PTY',async()=>{
 const first={session:'a'.repeat(32),epoch:1,generation:1},next={...first,generation:2};
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const frame={...raw,session:first.session};
 const incomplete=JSON.parse(channelSnapshot(frame));
 incomplete.rows[0].image_slices=[{column:0,id:'1',slice:0}];
 const painted=[],received=[],acks=[];
 const h=harness(async(name,args)=>{
  switch(name){
   case 'open_shell':return first;
   case 'subscribe_local_shell':received.push(args.stream.onmessage);return null;
   case 'recover_shell':assert.equal(args.epoch,1);assert.equal(args.frameVersion,CHANNEL_RENDITION_VERSION);
    return {owner:next,frame:channelSnapshot(frame),input_allowed:true};
   case 'ack_shell':acks.push(args.epoch);return null;
   default:throw Error('unexpected local IPC '+name);
  }
 },undefined,undefined,(_canvas,frame)=>painted.push(frame.revision),{},testChannel());
 await h.click('#open');
 await received[0]({owner:first,kind:'update',sequence:'1',frame:JSON.stringify(incomplete),status:channelHealth(1)});
 assert.equal(painted.length,0);assert.equal(h.el('#input').disabled,true);assert.deepEqual(acks,[]);
 await h.click('#recover');
 assert.deepEqual(acks,[1]);assert.deepEqual(painted,[String(frame.revision)]);
 assert.equal(received.length,2);assert.equal(h.el('#input').disabled,false);
});

test('split observations paint the inactive shell and cannot claim live input',async()=>{
 const a={session:'a'.repeat(32),epoch:1,generation:1},b={session:'b'.repeat(32),epoch:2,generation:2};
 const returned={...a,generation:3};
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const frame=owner=>({...raw,session:owner.session,epoch:owner.epoch});
 const events=[],observers=[],painted=[],adopted=[],writes=[];
 const h=harness(async(name,args)=>{
  switch(name){
   case 'open_shell':return a;
   case 'subscribe_local_shell':events.push(args.stream.onmessage);return null;
   case 'set_local_observer':observers.push(args.observer);return null;
   case 'ack_shell':case 'ack_local_stream':return null;
   case 'activate_local_tab':{
    const owner=args.targetEpoch===null?b:returned;
    adopted.push(owner.epoch);
    assert.equal(args.frameVersion,CHANNEL_RENDITION_VERSION);
    return {Ok:{owner,frame:channelSnapshot(frame(owner)),input_allowed:true}};
   }
   case 'write_shell':writes.push(args.epoch);return args.bytes.length;
   default:throw Error('unexpected local IPC '+name);
  }
 },undefined,undefined,(canvas,frame)=>painted.push({canvas,epoch:frame.epoch}),{},testChannel());
 await h.click('#open');await h.click('#new-local-tab');await h.click('#split-local');
 const layout=observers[0].layout;
 assert.equal(observers[0].epoch,1);assert.equal(h.el('#terminal-panes').dataset.split,'right');
 await events[1]({owner:b,kind:'update',sequence:'1',frame:null,preview:{layout,frame:channelSnapshot(frame(a))},status:channelHealth(2)});
 assert.equal(painted.at(-1).canvas,h.el('#peer-canvas'));assert.equal(painted.at(-1).epoch,1);
 assert.deepEqual(adopted,[2]);
 h.el('#input').value='send B';await h.submit();
 await h.click('#focus-peer-pane');
 assert.equal(h.el('#terminal-panes').dataset.split,'left');
 assert.equal(h.el('#peer-canvas').dataset.geometry,undefined);
 await events[1]({owner:b,kind:'update',sequence:'2',frame:null,preview:{layout,frame:channelSnapshot(frame(a))},status:channelHealth(2)});
 assert.equal(h.el('#peer-canvas').dataset.geometry,undefined);
 await events[2]({owner:returned,kind:'update',sequence:'1',frame:null,preview:{layout,frame:channelSnapshot(frame(b))},status:channelHealth(1)});
 assert.equal(painted.at(-1).canvas,h.el('#peer-canvas'));assert.equal(painted.at(-1).epoch,2);
 h.el('#input').value='send A';await h.submit();
 assert.deepEqual(adopted,[2,1]);assert.deepEqual(writes,[2,1]);
});

test('three visible shells rotate a single writer and ACK a retired preview on the same stream',async()=>{
 const own=(epoch,generation)=>({session:String(epoch).repeat(32),epoch,generation});
 const a=own(1,1),b=own(2,2),c=own(3,3),aAgain=own(1,4),d=own(4,5);
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const frame=(owner,revision=raw.revision)=>channelSnapshot({...raw,session:owner.session,epoch:owner.epoch,revision});
 const events=[],layouts=[],painted=[],acks=[],writes=[];
 const opening=[b,c,d];
 const h=harness(async(name,args)=>{
  switch(name){
   case 'open_shell':return a;
   case 'subscribe_local_shell':events.push({handle:args.stream.onmessage,epoch:args.owner.epoch});return null;
   case 'set_local_observers':
    assert.equal(args.observers.length,2);
    assert.ok(args.observers.every(target=>target.epoch!==args.owner.epoch));
    layouts.push({epoch:args.owner.epoch,targets:args.observers});return null;
   case 'set_local_observer':return null;
   case 'ack_shell':return null;
   case 'ack_local_stream':acks.push([args.owner.epoch,args.sequence,args.revision]);return null;
   case 'write_shell':writes.push(args.epoch);return args.bytes.length;
   case 'activate_local_tab':{
    assert.ok(args.targetEpoch===null||args.targetEpoch===1);
    const next=args.targetEpoch===1?aAgain:opening.shift();
    return {Ok:{owner:next,frame:frame(next),input_allowed:true}};
   }
   default:throw Error('unexpected local IPC '+name);
  }
 },undefined,undefined,(canvas,next)=>painted.push({canvas,epoch:next.epoch,rev:next.revision}),{},testChannel(),{localStorage:{getItem:key=>key==='aether.preview-three-panes'?'1':null}});
 const event=(owner,seq,preview,active=null)=>({owner,kind:'update',sequence:String(seq),frame:active,preview,status:channelHealth(owner.epoch)});
 await h.click('#open');await h.click('#new-local-tab');await h.click('#new-local-tab');
 assert.equal(h.el('#split-three-local').disabled,false);
 await h.click('#split-three-local');
 assert.equal(h.el('#terminal-panes').dataset.triple,'true');
 assert.equal(h.el('#active-pane').dataset.slot,'2');
 const [slotA,slotB]=layouts.at(-1).targets;
 assert.deepEqual([slotA.epoch,slotB.epoch],[1,2]);
 const sendC=events.at(-1).handle;
 await sendC(event(c,1,{layout:slotA.layout,frame:frame(a,17)},frame(c)));
 await sendC(event(c,2,{layout:slotB.layout,frame:frame(b,2)}));
 assert.deepEqual(painted.slice(-2).map(item=>[item.canvas===h.el('#preview-canvas-1')?1:2,item.epoch,item.rev]),[[1,1,'17'],[2,2,'2']]);
 h.el('#input').value='write only C';await h.submit();
 await h.click('#split-three-local');await h.click('#split-three-local');
 const newerA=layouts.at(-1).targets[0];
 assert.notEqual(newerA.layout,slotA.layout);
 const count=painted.length;
 await sendC(event(c,3,{layout:slotA.layout,frame:'obsolete wrong frame'}));
 assert.equal(painted.length,count);
 assert.equal(h.el('#input').disabled,false);
 assert.deepEqual(acks,[[3,'1',String(raw.revision)],[3,'2',null],[3,'3',null]]);
 await h.click('#focus-preview-pane-1');
 assert.equal(events.at(-1).epoch,1);
 assert.equal(h.el('#active-pane').dataset.slot,'0');
 assert.deepEqual(Array.from(layouts.at(-1).targets,target=>target.epoch),[2,3]);
 h.el('#input').value='write only A';await h.submit();
 await h.click('#new-local-tab');
 assert.equal(events.at(-1).epoch,4);
 assert.equal(h.el('#active-pane').dataset.slot,'0');
 assert.deepEqual(Array.from(layouts.at(-1).targets,target=>target.epoch),[2,3]);
 assert.equal(h.el('#local-tab-list').options.length,4);
 assert.equal(h.el('#preview-pane-1').hidden,false);
 assert.equal(h.el('#preview-pane-2').hidden,false);
 assert.deepEqual(writes,[3,1]);
});

test('directional palette actions move the single writer by visible layout, not the tab roster',async()=>{
 const own=(epoch,generation)=>({session:String(epoch).repeat(32),epoch,generation});
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const activations=[],writes=[],pending=deferred();
 let blockWrite=true;
 let opened=1,generation=1;
 const h=harness(async(name,args)=>{
  switch(name){
   case 'open_shell':return own(1,1);
   case 'activate_local_tab':{
    const next=args.targetEpoch??++opened, owner=own(next,++generation);
    activations.push(next);
    return {Ok:{owner,frame:channelSnapshot({...raw,session:owner.session,epoch:next}),input_allowed:true}};
   }
   case 'write_shell':
    writes.push(args.epoch);
    if(blockWrite)await pending.promise;
    return args.bytes.length;
   default:return null;
  }
 },undefined,undefined,undefined,{},testChannel(),
 {localStorage:{getItem:key=>key==='aether.preview-three-panes'?'1':null}});
 let focused=0;
 h.el('#ime-input').focus=()=>focused++;
 const choose=async(direction,active)=>{
  h.click('#command-palette-open');
  const query=h.el('#command-palette-query'), dialog=h.el('#command-palette');
  query.value='focus pane '+direction;query.handlers.input();
  const results=h.el('#command-palette-results').options;
  assert.equal(results.length,1);
  const button=results[0].children[0];
  assert.equal(button.getAttribute('aria-disabled'),active?null:'true');
  const count=activations.length, previous=focused;
  await button.click();await tick();
  assert.equal(activations.length,count+(active?1:0));
  assert.equal(dialog.open,!active);
  if(active)assert.ok(focused>previous,'the selected pane owns keyboard focus');
  else h.click('#command-palette-open');
 };
 await h.click('#open');
 for(let i=0;i<3;i++)await h.click('#new-local-tab');
 await h.click('#split-three-local'); // Shell 4 is lower-right; shell 3 is parked.
 const input=h.el('#input');
 input.value='write before switching from Shell 4';
 const receipt=h.submit();
 await tick();
 const query=h.el('#command-palette-query'), dialog=h.el('#command-palette');
 const list=h.el('#command-palette-results');
 list.querySelectorAll=()=>list.options.map(item=>item.children[0]);
 h.click('#command-palette-open');
 query.value='focus pane up';query.handlers.input();
 const button=list.querySelectorAll()[0];
 let cursor=query;
 button.focus=()=>{cursor=button;};
 input.focus=()=>{cursor=input;};
 button.focus();
 assert.equal(button.getAttribute('aria-disabled'),'true');
 assert.equal(button.children[0].children[0].textContent,'Wait for the current terminal input to finish.');
 const enter=()=>dialog.handlers.keydown({key:'Enter',target:cursor,preventDefault(){}});
 enter();await tick();
 assert.equal(dialog.open,true);
 assert.equal(input.value,'write before switching from Shell 4');
 assert.equal(h.el('#active-pane').dataset.slot,'2');
 assert.deepEqual(writes,[4]);
 blockWrite=false;pending.resolve();await receipt;
 assert.equal(input.value,'');
 assert.equal(dialog.open,true);
 assert.equal(query.value,'focus pane up');
 assert.equal(list.querySelectorAll()[0],button);
 assert.ok(cursor===button,'write completion must not steal focus from the open palette');
 assert.equal(button.getAttribute('aria-disabled'),null);
 enter();await tick();
 assert.equal(dialog.open,false);
 assert.equal(activations.at(-1),2);
 await choose('right',false);
 await choose('left',true);assert.equal(activations.at(-1),1);
 await choose('down',false);
 await choose('right',true);assert.equal(activations.at(-1),2);
 h.el('#input').value='write only to the selected pane';await h.submit();
 assert.deepEqual(writes,[4,2]);
 assert.equal(h.el('#terminal-panes').dataset.triple,'true');
 await h.click('#split-three-local'); // A non-split tab roster offers no directions.
 await choose('left',false);
 await h.click('#split-local');
 await choose('up',false);
 await choose('left',true);assert.equal(activations.at(-1),1);
 assert.equal(h.el('#terminal-panes').dataset.split,'left');
});

test('directional palette stays blocked until both accepted terminal key receipts settle',async()=>{
 const own=(epoch,generation)=>({session:String(epoch).repeat(32),epoch,generation});
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const [first,second]=[deferred(),deferred()];
 const keys=[],activations=[];
 let generation=1;
 const h=harness(async(name,args)=>{
  if(name==='open_shell')return own(1,1);
  if(name==='activate_local_tab'){
   const target=args.targetEpoch??2,owner=own(target,++generation);
   activations.push(target);
   return {Ok:{owner,frame:channelSnapshot({...raw,session:owner.session,epoch:target}),input_allowed:true}};
  }
  if(name==='key_shell'){
   keys.push([args.epoch,args.name]);
   return args.name==='a'?first.promise:second.promise;
  }
  return null;
 },undefined,undefined,undefined,{},testChannel());
 await h.click('#open');await h.click('#new-local-tab');await h.click('#split-local');
 for(const key of ['a','b'])h.el('canvas').handlers.keydown({key,preventDefault(){}});
 await tick();
 const query=h.el('#command-palette-query'), dialog=h.el('#command-palette');
 const list=h.el('#command-palette-results');
 list.querySelectorAll=()=>list.options.map(item=>item.children[0]);
 h.click('#command-palette-open');
 query.value='focus pane left';query.handlers.input();
 const result=list.querySelectorAll()[0];
 const enter=()=>dialog.handlers.keydown({key:'Enter',target:query,preventDefault(){}});
 assert.deepEqual(keys,[[2,'a']]);
 assert.equal(result.getAttribute('aria-disabled'),'true');
 enter();await tick();
 assert.equal(dialog.open,true);assert.deepEqual(activations,[2]);
 first.resolve();await tick();
 assert.deepEqual(keys,[[2,'a'],[2,'b']]);
 assert.equal(result.getAttribute('aria-disabled'),'true');
 enter();await tick();
 assert.equal(dialog.open,true);assert.deepEqual(activations,[2]);
 second.resolve();await tick();
 assert.equal(dialog.open,true);
 assert.equal(query.value,'focus pane left');
 assert.equal(list.querySelectorAll()[0],result);
 assert.equal(result.getAttribute('aria-disabled'),null);
 enter();await tick();
 assert.equal(dialog.open,false);assert.deepEqual(activations,[2,1]);
 assert.deepEqual(keys,[[2,'a'],[2,'b']]);
});

for(const [closed,expectedSplit,expectedPeer] of [[3,'right',1],[1,'top',3]]){
 test(`closing visible shell ${closed} preserves the other two panes and never selects parked shell 4`,async()=>{
  const own=(epoch,generation)=>({session:String(epoch).repeat(32),epoch,generation});
  const a=own(1,1),b=own(2,2),c=own(3,3),d=own(4,4);
  const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
  const frame=owner=>channelSnapshot({...raw,session:owner.session,epoch:owner.epoch});
  const opening=[b,c,d], seen=[], previews=[],writes=[];
  let generation=4;
  const h=harness(async(name,args)=>{
   switch(name){
    case 'open_shell':return a;
    case 'subscribe_local_shell':if(args.observer)previews.push(args.observer.epoch);return null;
    case 'ack_shell':return null;
    case 'set_local_observers':return null;
    case 'set_local_observer':if(args.observer)previews.push(args.observer.epoch);return null;
    case 'close_shell':seen.push(['close',args.epoch]);return {cleanup_pending:false};
    case 'write_shell':writes.push(args.epoch);return args.bytes.length;
    case 'activate_local_tab':{
     const next=args.targetEpoch===null?opening.shift():own(args.targetEpoch,++generation);
     seen.push(['activate',args.owner?.epoch??null,next.epoch]);
     return {Ok:{owner:next,frame:frame(next),input_allowed:true}};
    }
    default:throw Error('unexpected local IPC '+name);
   }
  },undefined,undefined,undefined,{},testChannel(),{localStorage:{getItem:key=>key==='aether.preview-three-panes'?'1':null}});
  await h.click('#open');
  for(let i=0;i<3;i++)await h.click('#new-local-tab');
  await h.el('#local-tab-list').options.find(tab=>tab.dataset.epoch===String(closed)).handlers.click();
  await tick();await tick();
  await h.click('#split-three-local');
  assert.equal(h.el('#terminal-panes').dataset.triple,'true');
  await h.click('#close');
  assert.deepEqual(seen.slice(-2),[['close',closed],['activate',null,2]]);
  assert.equal(h.el('#terminal-panes').dataset.triple,'');
  assert.equal(h.el('#terminal-panes').dataset.split,expectedSplit);
  assert.equal(previews.at(-1),expectedPeer);
  assert.equal(h.el('#input').disabled,false);
  h.el('#input').value='surviving shell only';await h.submit();
  assert.deepEqual(writes,[2]);
 });
}

for(const [removed,expectedSplit,expectedPeer] of [[3,'right',1],[1,'top',3]]){
 test(`refresh dropping passive shell ${removed} retains the active writer and exact surviving split`,async()=>{
  const own=(epoch,generation)=>({session:String(epoch).repeat(32),epoch,generation});
  const [a,b,c,d]=[1,2,3,4].map(epoch=>own(epoch,epoch)),returned=own(2,5);
  const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
  const frame=owner=>channelSnapshot({...raw,session:owner.session,epoch:owner.epoch});
  const opening=[b,c,d],seen=[],acks=[],painted=[],targets=[];
  const listeners=[];
  const h=harness(async(name,args)=>{
   switch(name){
    case 'open_shell':return a;
    case 'subscribe_local_shell':listeners.push(args.stream.onmessage);return null;
    case 'ack_shell':return null;
    case 'set_local_observers':targets.push(...args.observers);return null;
    case 'set_local_observer':targets.push(args.observer);return null;
    case 'list_local_shells':return [1,2,3,4].filter(value=>value!==removed);
    case 'ack_local_stream':acks.push([args.owner.epoch,args.sequence,args.revision]);return null;
    case 'write_shell':seen.push(['write',args.epoch]);return args.bytes.length;
    case 'activate_local_tab':{
     const next=args.targetEpoch===null?opening.shift():returned;
     seen.push(['activate',next.epoch]);return {Ok:{owner:next,frame:frame(next),input_allowed:true}};
    }
    default:throw Error('unexpected local IPC '+name);
   }
  },undefined,undefined,(canvas,frame)=>painted.push([canvas,frame.epoch]),{},testChannel(),
    {localStorage:{getItem:key=>key==='aether.preview-three-panes'?'1':null}});
  await h.click('#open');
  for(let i=0;i<3;i++)await h.click('#new-local-tab');
  h.el('#local-tab-list').options.find(tab=>tab.dataset.epoch==='2').handlers.click();await tick();
  await h.click('#split-three-local');
  assert.equal(h.el('#terminal-panes').dataset.triple,'true');
  const old=targets.find(target=>target?.epoch===removed);
  const listener=listeners.at(-1);
  const event=(seq,preview)=>({owner:returned,kind:'update',sequence:String(seq),frame:null,preview,status:channelHealth(2)});
  await listener(event(1,{layout:old.layout,frame:frame(removed===1?a:c)}));
  assert.equal(painted.at(-1)[1],removed);
  const size=seen.length;
  h.el('#input').value='unsent draft on B';
  await h.click('#manage-local');
  assert.equal(h.el('#terminal-panes').dataset.triple,'');
  assert.equal(h.el('#terminal-panes').dataset.split,expectedSplit);
  assert.equal(targets.at(-1).epoch,expectedPeer);
  assert.notEqual(targets.at(-1).layout,old.layout);
  assert.equal(h.el('#input').value,'unsent draft on B');
  assert.equal(h.el('#input').disabled,false);
  assert.equal(h.el('#local-tab-list').options.find(tab=>tab.dataset.epoch==='2').attributes.get('aria-current'),'true');
  assert.ok(h.el('#local-tab-list').options.some(tab=>tab.dataset.epoch==='4'));
  assert.equal(seen.length,size);
  const paints=painted.length;
  await listener(event(2,{layout:old.layout,frame:'obsolete malformed preview'}));
  assert.equal(painted.length,paints);
  assert.deepEqual(acks,[[2,'1',null],[2,'2',null]]);
  await listener(event(3,{layout:targets.at(-1).layout,frame:frame(expectedPeer===1?a:c)}));
  assert.deepEqual(painted.at(-1),[h.el('#peer-canvas'),expectedPeer]);
  await h.submit();
  assert.deepEqual(seen.slice(size),[['write',2]]);
 });
}

test('SSH terminal recovers the same grid on a new generation and reattaches read-only',async()=>{
 const old={session:'a'.repeat(32),epoch:1,generation:1},next={...old,generation:2},readonly={...old,generation:4};
 const frame={...JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8')),session:old.session,epoch:1};
 const seen=[];let first=true,linkLost=false;
 const h=harness(async(name,{action})=>{
  assert.equal(name,'ssh_terminal');seen.push(action);
  if(action.operation==='open'){assert.equal(action.destination,'ubuntu@192.0.2.10');return old;}
  if(action.operation==='attach')return {owner:readonly,frame};
  const c=action.command;
  switch(c.kind){
   case 'poll':if(linkLost)throw Error('SSH link ended');if(first){first=false;return {kind:'snapshot',frame};}return null;
   case 'status':return {epoch:1,exited:false,drained:false,write_error:null,unsupported:'0',dropped_reply_bytes:'0'};
   case 'ack':case 'detach':return null;
   case 'close':return {cleanup_pending:false};
   case 'recover':assert.equal(c.owner.generation,1);linkLost=false;return {owner:next,frame,input_allowed:true};
   case 'write':assert.equal(c.owner.generation,2);return c.bytes.length;
   default:throw Error(c.kind);
  }
 });
 h.el('#ssh-terminal-host').value='ubuntu@192.0.2.10';await h.submitSsh();
 assert.equal(h.el('#input').disabled,false);
 linkLost=true;await [...h.timers.values()][0]();assert.equal(h.el('#input').disabled,true);
 assert.match(h.el('#status').textContent,/SSH terminal paused/);
 await h.click('#recover');assert.equal(h.el('#input').disabled,false);
 h.el('#input').value='echo ready';await h.submit();
 assert.equal(seen.filter(a=>a.command?.kind==='write').length,1);
 await h.click('#detach');assert.equal(h.el('#open-ssh').disabled,true);
 await h.click('#attach');assert.equal(h.el('#input').disabled,true);
 assert.match(h.el('#status').textContent,/SSH shell in view-only mode/);
 await h.click('#close');assert.equal(seen.at(-1).command.owner.generation,4);
 assert.equal(h.el('#open-ssh').disabled,false);
});

test('a fresh WebView selects a saved SSH shell explicitly and acknowledges it read-only',async()=>{
 const saved={id:'a'.repeat(32),destination:'remembered-host',created_ms:1};
 const owner={session:'b'.repeat(32),epoch:1,generation:4};
 const frame={...JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8')),session:owner.session,epoch:1};
 const calls=[];
 const h=harness(async(name,{action})=>{
  assert.equal(name,'ssh_terminal');calls.push(action);
  if(action.operation==='list_saved')return [saved];
  if(action.operation==='restore_saved'){assert.equal(action.id,saved.id);return {owner,frame};}
  switch(action.command.kind){
   case 'ack':assert.equal(action.command.owner.generation,4);assert.equal(action.command.revision,frame.revision);return null;
   case 'poll':return null;
   case 'status':return {epoch:1,exited:false,drained:false,write_error:null,unsupported:'0',dropped_reply_bytes:'0'};
   default:throw Error(action.command.kind);
  }
 });
 assert.equal(calls.length,0);
 await h.click('#show-saved-ssh');
 assert.deepEqual(calls.map(call=>call.operation),['list_saved']);
 await h.click('#resume-saved-ssh');
 assert.deepEqual(calls.map(call=>call.command?.kind??call.operation),['list_saved','restore_saved','ack','poll','status']);
 assert.equal(h.el('#resize').disabled,true);
 assert.equal(h.el('#input').disabled,true);
 h.el('#input').value='echo must stay local';await h.submit();
 assert.equal(calls.filter(call=>call.command?.kind==='write').length,0);
 assert.match(h.el('#status').textContent,/remembered-host in view-only mode/);
});

test('a saved SSH shell that expired releases the open controls',async()=>{
 const saved={id:'a'.repeat(32),destination:'expired-host',created_ms:1};
 const h=harness(async(name,{action})=>{
  assert.equal(name,'ssh_terminal');
  if(action.operation==='list_saved')return [saved];
  assert.equal(action.operation,'restore_saved');
  return {ended:true,message:'The saved SSH shell has expired or was closed.'};
 });
 await h.click('#show-saved-ssh');await h.click('#resume-saved-ssh');
 assert.match(h.el('#status').textContent,/expired or was closed/);
 assert.equal(h.el('#open-ssh').disabled,false);
});

test('SSH recovery preserves the lock after an uncertain write and never replays it',async()=>{
 const owner={session:'b'.repeat(32),epoch:1,generation:1};
 const frame={...JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8')),session:owner.session,epoch:1};
 let writes=0;
 const h=harness(async(name,{action})=>{
  assert.equal(name,'ssh_terminal');
  if(action.operation==='open')return owner;
  switch(action.command.kind){
   case 'poll':case 'ack':return null;
   case 'status':return {epoch:1,exited:false,drained:false,write_error:null,unsupported:'0',dropped_reply_bytes:'0'};
   case 'write':writes++;throw Error('SSH terminal disconnected');
   case 'recover':return {owner:{...owner,generation:2},frame,input_allowed:false};
   default:throw Error(action.command.kind);
  }
 });
 h.el('#ssh-terminal-host').value='lab';await h.submitSsh();
 h.el('#input').value='echo once';await h.submit();
 await h.click('#recover');
 assert.equal(h.el('#input').disabled,true);assert.equal(writes,1);
});

test('an expired detached SSH session frees the host field for a new connection',async()=>{
 const owner={session:'c'.repeat(32),epoch:1,generation:1};
 const h=harness(async(name,{action})=>{
  assert.equal(name,'ssh_terminal');
  if(action.operation==='open')return owner;
  if(action.operation==='attach')return null;
  switch(action.command.kind){
   case 'poll':case 'detach':return null;
   case 'status':return {epoch:1,exited:false,drained:false,write_error:null,unsupported:'0',dropped_reply_bytes:'0'};
   default:throw Error(action.command.kind);
  }
 });
 h.el('#ssh-terminal-host').value='lab';await h.submitSsh();await h.click('#detach');await h.click('#attach');
 assert.equal(h.el('#open-ssh').disabled,false);assert.equal(h.el('#ssh-terminal-host').disabled,false);
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
 assert.match(h.el('#status').textContent,/Local \/bin\/sh/);assert.equal(h.el('#demo').disabled,true);
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
 assert.match(h.el('#status').textContent,/Local \/bin\/sh/);assert.equal(h.el('#resize').disabled,false);
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

test('detach and reconnect restores the same shell only as a view',async()=>{
 const frame=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 frame.session='00000000000000000000000000000001';
 const seen=[];
 const h=harness(async(name,args)=>{
  seen.push(name);
  if(name==='open_shell')return 1;
  if(name==='attach_shell')return {owner:{session:frame.session,epoch:1,generation:3},frame};
  return null;
 });
 await h.click('#open');await h.click('#detach');
 assert.equal(h.timers.size,0);assert.match(h.el('#status').textContent,/View detached/);
 await h.click('#attach');
 assert.equal(h.timers.size,1);assert.match(h.el('#status').textContent,/view-only/);
 assert.equal(h.el('#input').disabled,true);assert.equal(h.el('#resize').disabled,true);
 h.el('canvas').handlers.keydown({key:'a'});await tick();
 assert.ok(seen.includes('detach_shell')&&seen.includes('ack_shell'));
 assert.ok(!seen.includes('key_shell')&&!seen.includes('close_shell'));
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
 assert.equal(writes.length,1);assert.equal(writes[0].epoch,1);assert.match(h.el('#status').textContent,/Local \/bin\/sh/);
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
 assert.equal(h.el('#input').value,'fresh');assert.equal(h.el('#input').disabled,false);assert.match(h.el('#status').textContent,/Local \/bin\/sh/);
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

test('visible grid copy removes only ASCII padding, preserving deliberate Unicode spaces',async()=>{
 const frame=JSON.parse(fs.readFileSync(new URL('../web/unicode-frame.json',import.meta.url),'utf8'));
 frame.session='pty-demo';frame.cols=5;frame.cursor_row=0;frame.cursor_col=0;
 const color={foreground:'#ffffff',background:'#000000',style:0};
 const c=(text,width=1)=>({...color,text,width});
 frame.rows=[
  [c('A'),c('\u00a0'),c(' '),c(' '),c(' ')],
  [c('界',2),c(' ',0),c('\u3000',2),c(' ',0),c(' ')],
  [c('B'),c(' '),c('\u202f'),c(' '),c(' ')],
  [c(' '),c(' '),c(' '),c(' '),c(' ')]
 ];
 const seen=[];
 const h=harness(async(name,args)=>{
  if(name==='open_shell')return 1;
  if(name==='poll_shell')return frame;
  if(name==='ack_shell'){seen.push(args.revision);return;}
  assert.fail('unexpected IPC '+name);
 });
 await h.click('#open');h.click('#copy-visible');
 assert.deepEqual(seen,[frame.revision]);
 assert.equal(h.el('#history-output').value,'A\u00a0\n界\u3000\nB \u202f\n');
});

test('live paint failure is not ACKed; same-geometry recovery and detach retain their semantics',async()=>{
 const initial=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 initial.session='1'.padStart(32,'0');
 const cells=initial.rows[0].map((c,i)=>i===0?{...c,text:'X'}:c);
 const delta={kind:'delta',frame:{...initial,base_revision:1,revision:2,rows:[{index:0,cells}]}};
 const recovered={...initial,revision:3,rows:[cells,...initial.rows.slice(1)]};
 const acknowledgements=[], resizes=[];
 let polls=0, fail=false, pixels=[], clip=0, width=300, height=150;
 const h=harness(async(name,args)=>{
  if(name==='open_shell')return 1;
  if(name==='poll_shell'){polls++;return polls===1?initial:polls===2?delta:null;}
  if(name==='recover_shell')return recovered;
  if(name==='detach_shell')return;
  if(name==='ack_shell'){
   assert.equal(clip,0);assert.equal(pixels.length,initial.rows.length*initial.cols);
   acknowledgements.push([args.revision,pixels[0]]);
   return;
  }
  throw Error(name);
 },undefined,undefined,paintFrame);
 const canvas=h.el('canvas');
 Object.defineProperties(canvas,{
  width:{get:()=>width,set:x=>{width=x;pixels=[];clip=0;resizes.push('width');}},
  height:{get:()=>height,set:x=>{height=x;pixels=[];clip=0;resizes.push('height');}},
 });
 canvas.getContext=()=>({
  setTransform(){},scale(){},clearRect(){assert.equal(clip,0);pixels=[];},
  fillRect(){},beginPath(){},rect(){},clip(){},strokeRect(){},
  save(){clip++;},restore(){clip--;},
  fillText(text){if(fail)throw Error('paint interrupted');pixels.push(text);},
 });
 await h.click('#open');
 assert.deepEqual(acknowledgements,[[1,initial.rows[0][0].text]]);
 fail=true;await [...h.timers.values()][0]();
 assert.match(h.el('#status').textContent,/paint interrupted/);
 assert.deepEqual(acknowledgements,[[1,initial.rows[0][0].text]]);
 assert.equal(h.el('#input').disabled,true);
 fail=false;await h.click('#recover');
 assert.deepEqual(acknowledgements,[[1,initial.rows[0][0].text],[3,'X']]);
 assert.equal(h.el('#input').disabled,true,'local recovery does not clear input safety locks');
 assert.deepEqual(resizes,['width','height'],'same-geometry update and recovery retain buffers');
 await h.click('#detach');
 assert.deepEqual(resizes,['width','height','width'],'detach still explicitly discards pixels');
 assert.equal(pixels.length,0);
});

test('same-epoch SSH recovery retires old history requests without losing the committed archive page',async()=>{
 const first={session:'d'.repeat(32),epoch:1,generation:1};
 const resumed={...first,generation:2};
 const frame={...JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8')),
  session:first.session,epoch:first.epoch};
 const requests=[],selectionWrites=[];
 const h=harness(async(name,{action})=>{
  assert.equal(name,'ssh_terminal');
  if(action.operation==='open')return first;
  const command=action.command;
  switch(command.kind){
   case 'poll':case 'ack':return null;
   case 'status':return {epoch:1,exited:false,drained:false,write_error:null,unsupported:'0',dropped_reply_bytes:'0'};
   case 'recover':assert.deepEqual(command.owner,first);return {owner:resumed,frame,input_allowed:true};
   case 'history':{
    const pending=deferred();requests.push({command,pending});return pending.promise;
   }
   default:throw Error(command.kind);
  }
 });
 const output=h.el('#history-output'),label=h.el('#history-label');
 output.setSelectionRange=(...range)=>selectionWrites.push(range);
 h.el('#ssh-terminal-host').value='lab';
 await h.submitSsh();
 function result(text,offset,revision='12'){
  return {epoch:1,revision,offset,total_rows:300,text};
 }
 const initial=h.click('#history-refresh');
 requests[0].pending.resolve(result('initial',0));await initial;
 const committed=h.click('#history-older');
 requests[1].pending.resolve(result('retained\u00a0page',20));await committed;
 const retainedLabel=label.textContent;
 const stale=h.click('#history-older');
 assert.equal(requests[2].command.offset,40);
 assert.deepEqual(requests[2].command.owner,first);
 await h.click('#recover');
 assert.equal(output.value,'retained\u00a0page');
 assert.deepEqual(selectionWrites,[[0,0],[0,0]]);

 const fresh=h.click('#history-older');
 assert.equal(requests.length,4,'a retired attachment must not block the current request');
 assert.equal(requests[3].command.offset,40,'new intent starts from the committed offset');
 assert.deepEqual(requests[3].command.owner,resumed);
 requests[2].pending.resolve(result('late old attachment',40,'13'));
 await stale;
 assert.equal(output.value,'retained\u00a0page');
 assert.equal(label.textContent,retainedLabel);
 assert.deepEqual(selectionWrites,[[0,0],[0,0]]);
 requests[3].pending.resolve(result('current attachment',40,'14'));await fresh;
 assert.equal(output.value,'current attachment');
 assert.deepEqual(selectionWrites,[[0,0],[0,0],[0,0]]);
});

function sameEpochInputHarness(onInput) {
 const first={session:'e'.repeat(32),epoch:1,generation:1};
 const attached={...first,generation:3};
 const frame={...JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8')),session:first.session};
 const commands=[];
 const h=harness(async(name,{action})=>{
  assert.equal(name,'ssh_terminal');
  if(action.operation==='open')return first;
  if(action.operation==='attach')return {owner:attached,frame};
  const command=action.command;commands.push(command);
  switch(command.kind){
   case 'recover':return {owner:{...first,generation:2},frame,input_allowed:true};
   case 'write':case 'key':return onInput(command);
   case 'poll':case 'ack':case 'detach':return null;
   case 'status':return {epoch:1,exited:false,drained:false,write_error:null,unsupported:'0',dropped_reply_bytes:'0'};
   default:throw Error(command.kind);
  }
 });
 async function open(){h.el('#ssh-terminal-host').value='lab';await h.submitSsh();}
 async function reattach(){await h.click('#detach');await h.click('#attach');}
 return {...h,open,reattach,commands};
}

test('late command ACK cannot clear or refocus the same-epoch read-only reattachment',async()=>{
 const old=deferred();
 const h=sameEpochInputHarness(command=>{assert.equal(command.owner.generation,1);return old.promise;});
 let focused=0;
 const input=h.el('#input');input.focus=()=>focused++;
 await h.open();input.value='old write';
 const submit=h.submit();await tick();
 const write=h.commands.find(({kind})=>kind==='write');assert.ok(write);
 await h.reattach();input.value='retained draft';
 old.resolve(write.bytes.length);await submit;
 assert.equal(input.value,'retained draft');
 assert.equal(focused,0);
 assert.equal(input.disabled,true);
 assert.match(h.el('#status').textContent,/view-only mode/);
 assert.equal(h.commands.filter(({kind})=>kind==='write').length,1);
});

for(const result of ['ok','error']){
 test(`retired logical input ${result} cannot dispatch queued keys or change the reattached UI`,async()=>{
  const old=deferred(),keys=[];
  const h=sameEpochInputHarness(command=>{
   keys.push([command.name,command.owner.generation]);
   if(command.name==='a')return old.promise;
   return null;
  });
  await h.open();
  const key=name=>h.el('canvas').handlers.keydown({key:name,preventDefault(){}});
  key('a');await tick();key('b');
  assert.deepEqual(keys,[['a',1]]);
  await h.reattach();
  if(result==='ok')old.resolve(null);else old.reject(Error('old writer closed'));
  await tick();await tick();
  assert.deepEqual(keys,[['a',1]]);
  assert.equal(h.el('#input').disabled,true);
  assert.match(h.el('#status').textContent,/view-only mode/);
 });
}

test('same-shell recovery keeps pending-write safety lock without retrying a stale suffix',async()=>{
 const old=deferred();
 const h=sameEpochInputHarness(()=>old.promise);
 const input=h.el('#input');
 await h.open();input.value='old partial write';
 const submit=h.submit();await tick();
 assert.equal(h.commands.filter(({kind})=>kind==='write').length,1);
 await h.click('#recover');
 assert.match(h.el('#status').textContent,/SSH output snapshot recovered/);
 assert.equal(input.disabled,true);
 old.resolve(1);await submit;
 assert.equal(h.commands.filter(({kind})=>kind==='write').length,1);
 assert.equal(input.value,'old partial write');
 assert.equal(input.disabled,true);
 assert.equal(h.el('#send').disabled,true);
 assert.match(h.el('#status').textContent,/SSH output snapshot recovered/);
});

test('local reconnect targets exactly the detached PTY; startup still allows explicit singleton attach',async()=>{
 const first={session:'b'.repeat(32),epoch:1,generation:1};
 const resumed={...first,generation:3};
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const attachArgs=[],frames=[],acks=[];
 const h=harness(async(name,args)=>{
  switch(name){
   case 'open_shell':return first;
   case 'attach_shell':{
    attachArgs.push(args);
    if(attachArgs.length===1)return null;
    return {owner:resumed,frame:{...raw,session:resumed.session,epoch:resumed.epoch}};
   }
   case 'poll_shell':case 'detach_shell':return null;
   case 'ack_shell':acks.push(args);return null;
   default:throw Error(name);
  }
 },undefined,undefined,(_canvas,frame)=>frames.push(frame));
 await h.click('#attach');assert.equal(attachArgs[0],undefined);
 assert.match(h.el('#status').textContent,/No existing shell/);
 await h.click('#open');await h.click('#detach');await h.click('#attach');
 assert.equal(attachArgs.length,2);
 assert.equal(attachArgs[1].target.session,first.session);
 assert.equal(attachArgs[1].target.epoch,first.epoch);
 assert.deepEqual(Object.keys(attachArgs[1].target).sort(),['epoch','session']);
 assert.equal(frames.length,1);assert.equal(acks.length,1);
 assert.equal(h.el('#input').disabled,true);
 assert.match(h.el('#status').textContent,/view-only/);
});

test('local reconnect never paints or ACKs a foreign, stale, or missing retained target',async()=>{
 const first={session:'7'.repeat(32),epoch:1,generation:1};
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const expected={...first,generation:3};
 const replies=[{...expected,session:'8'.repeat(32)},{...expected,epoch:2},first,null,expected];
 const argsSeen=[],frames=[],acked=[];
 const h=harness(async(name,args)=>{
  switch(name){
   case 'open_shell':return first;
   case 'attach_shell':{
    argsSeen.push(args);
    const owner=replies.shift();
    return owner&&{owner,frame:{...raw,session:owner.session,epoch:owner.epoch}};
   }
   case 'poll_shell':case 'detach_shell':return null;
   case 'ack_shell':acked.push(args);return null;
   default:throw Error(name);
  }
 },undefined,undefined,(_canvas,frame)=>frames.push(frame));
 await h.click('#open');await h.click('#detach');
 for(let i=0;i<4;i++){
  await h.click('#attach');assert.match(h.el('#status').textContent,/retained local session/i);
  assert.equal(frames.length,0);assert.equal(acked.length,0);
 }
 await h.click('#attach');
 assert.equal(argsSeen.length,5);
 assert.ok(argsSeen.every(args=>args.target.session===first.session&&args.target.epoch===first.epoch));
 assert.equal(frames[0].session,first.session);
 assert.equal(acked.length,1);
 assert.equal(h.el('#input').disabled,true);
});

test('the live local roster switches directly to the selected shell view-only',async()=>{
 const first={session:'a'.repeat(32),epoch:1,generation:1};
 const second={session:'b'.repeat(32),epoch:2,generation:1};
 const frame=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const calls=[],painted=[],late=deferred();
 let firstOpened=false,secondOpened=false,secondPolls=0;
 const h=harness(async(name,args)=>{
  calls.push([name,args]);
  switch(name){
    case 'open_shell':assert(!firstOpened);firstOpened=true;return first;
    case 'list_local_shells':return (secondOpened?[first,second]:[first]).map(owner=>owner.epoch);
    case 'open_second_local_shell':assert.equal(args.epoch,first.epoch);secondOpened=true;return second;
    case 'poll_shell':return args.epoch===second.epoch&&++secondPolls===2?late.promise:null;
    case 'detach_shell':case 'ack_shell':return null;
    case 'attach_selected_local_shell':
      assert.equal(args.epoch,first.epoch);
      return {owner:{...first,generation:3},frame:{...frame,session:first.session,epoch:first.epoch}};
    case 'write_shell':assert.equal(args.epoch,second.epoch);return args.bytes.length;
    default:throw Error(name);
  }
 },undefined,undefined,(_canvas,frame)=>painted.push(frame.session));
 await h.click('#open');h.el('#input').value='first-only draft';
 await h.click('#manage-local');assert.equal(h.el('#open-second-local').disabled,true);
 await h.click('#detach');
 await h.click('#manage-local');assert.equal(h.el('#open-second-local').disabled,false);
 await h.click('#open-second-local');
 assert.equal(h.el('#input').value,'');assert.equal(h.el('#input').disabled,false);
 h.el('#input').value='second command';await h.submit();
 const oldPoll=[...h.timers.values()][0]();await tick();assert.equal(secondPolls,2);
 await h.click('#manage-local');
 assert.equal(h.el('#input').disabled,false);
 assert.equal(h.el('#attach-selected-local').disabled,true);
 assert.equal(h.el('#open-second-local').disabled,true);
 h.el('#local-shell-selection').value=String(second.epoch);h.el('#local-shell-selection').handlers.change();
 assert.equal(h.el('#attach-selected-local').disabled,true);
 h.el('#local-shell-selection').value=String(first.epoch);h.el('#local-shell-selection').handlers.change();
 await h.click('#attach-selected-local');
 late.resolve({...frame,session:second.session,epoch:second.epoch});await oldPoll;
 assert.deepEqual(painted,[first.session]);
 assert.equal(h.el('#input').value,'first-only draft');assert.equal(h.el('#input').disabled,true);
 assert.match(h.el('#status').textContent,/view-only/);
 assert.deepEqual(calls.filter(([name])=>name==='write_shell').map(([,args])=>args.epoch),[2]);
 assert.deepEqual(calls.filter(([name])=>['detach_shell','attach_selected_local_shell'].includes(name))
  .map(([name,args])=>[name,args.epoch]),[['detach_shell',1],['detach_shell',2],['attach_selected_local_shell',1]]);
});

test('editable tabs keep per-shell drafts and recover a lost snapshot ACK without reopening a PTY',async()=>{
 const a={session:'a'.repeat(32),epoch:1,generation:1},b={session:'b'.repeat(32),epoch:2,generation:2};
 const returned={...a,generation:3};
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const view=owner=>({Ok:{owner,frame:{...raw,session:owner.session,epoch:owner.epoch},input_allowed:true}});
 const activations=[],writes=[],acknowledged=[];
 let loseAck=true;
 const h=harness(async(name,args)=>{
  switch(name){
   case 'open_shell':return a;
   case 'poll_shell':return null;
   case 'activate_local_tab':activations.push(args);return view(args.targetEpoch===null?b:returned);
   case 'ack_shell':
    acknowledged.push(args.epoch);
    if(loseAck){loseAck=false;throw Error('ACK response lost');}
    return null;
   case 'write_shell':writes.push(args.epoch);return args.bytes.length;
   case 'text_shell':throw Error('stale IME commit was submitted');
   default:throw Error(name);
  }
 });
 const input=h.el('#input'),ime=h.el('#ime-input');
 await h.click('#open');input.value='draft A';ime.handlers.compositionstart();
 await h.click('#new-local-tab');
 assert.equal(input.disabled,true);assert.equal(h.el('#retry-local-tab').hidden,false);
 await h.click('#retry-local-tab');
 assert.equal(input.disabled,false);assert.equal(input.value,'');
 ime.handlers.compositionend({data:'late IME'});await tick();
 input.value='send B';await h.submit();input.value='draft B';
 const first=h.el('#local-tab-list').options.find(tab=>tab.dataset.epoch==='1');
 first.handlers.click();await tick();
 assert.equal(input.disabled,false);assert.equal(input.value,'draft A');
 await h.submit();
 assert.deepEqual(activations.map(({owner,targetEpoch,preserveInput})=>[owner.epoch,targetEpoch,preserveInput]),[[1,null,true],[2,1,true]]);
 assert.deepEqual(acknowledged,[2,2,1]);assert.deepEqual(writes,[2,1]);
});

function layoutStorage(layout){
 const entries=new Map();
 const environment={localStorage:{getItem:key=>entries.get(key),setItem:(key,value)=>entries.set(key,value),
  removeItem:key=>entries.delete(key)}};
 saveLocalShellLayout(layout,environment);
 return environment;
}

test('preview preference preserves the older record and detects a later older-build edit',()=>{
 const geometry={rows:24,cols:80},tab={...plainLocalTab,geometry};
 const legacy={tabs:[tab,tab],active:0,split:true,panes:[0,1]};
 const storage=layoutStorage(legacy);
 const key='aether.local-shell-layout.v5';
 const original=storage.localStorage.getItem(key);
 const preview={tabs:[tab,tab,tab,tab],active:2,triple:[2,0,1]};
 assert.equal(saveThreePaneLayout({...preview,triple:[2,0,0]},storage),false);
 assert.equal(saveThreePaneLayout(preview,storage),true);
 assert.deepEqual(readThreePaneLayout(storage)?.triple,[2,0,1]);
 assert.equal(storage.localStorage.getItem(key),original);
 assert.equal(saveLocalShellLayout({...legacy,active:1},storage),true);
 assert.equal(readThreePaneLayout(storage),null);
 assert.equal(readLocalShellLayout(storage).active,1);
 assert.equal(saveThreePaneLayout(preview,storage),true);
 assert.equal(saveThreePaneLayout(null,storage),true);
 assert.equal(readThreePaneLayout(storage),null);
 assert.equal(readLocalShellLayout(storage).active,1);
});

test('explicit preview restore brings back ordered visible slots and leaves the fourth tab parked',async()=>{
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const geometry={rows:raw.rows.length,cols:raw.cols};
 const legacy={tabs:[{...plainLocalTab,geometry}],active:0,split:false,panes:[]};
 const tabs=Array.from({length:4},()=>({...plainLocalTab,geometry,paneId:undefined}));
 const layout={tabs,active:0,triple:[2,0,1]};
 const storage=layoutStorage(legacy);
 storage.localStorage.setItem('aether.preview-three-panes','1');
 assert.equal(saveThreePaneLayout(layout,storage),true);
 const calls=[],owners=new Map();let next=10;
 const create=()=>{const epoch=++next;const owner={session:epoch.toString(16).repeat(32),epoch,generation:1};owners.set(epoch,owner);return owner;};
 const frame=owner=>channelSnapshot({...raw,session:owner.session,epoch:owner.epoch});
 const h=harness(async(name,args)=>{
  calls.push([name,args]);
  if(name==='list_local_shells')return [];
  if(name==='open_shell')return create();
  if(name==='activate_local_tab'){
   const before=owners.get(args.targetEpoch);
   const owner=before?{...before,generation:before.generation+1}:create();
   owners.set(owner.epoch,owner);
   return {Ok:{owner,frame:frame(owner),input_allowed:true}};
  }
  if(name==='subscribe_local_shell'){
   await args.stream.onmessage({owner:args.owner,kind:'update',sequence:'1',frame:frame(args.owner),
    preview:null,status:channelHealth(args.owner.epoch)});
   return null;
  }
  if(['ack_shell','ack_local_stream','set_local_observer','set_local_observers'].includes(name))return null;
  throw Error(name);
 },undefined,undefined,undefined,storage,testChannel(),storage);
 await tick();
 assert.equal(h.el('#restore-local-layout').hidden,false);
 assert.ok(!calls.some(([name])=>name==='open_shell'));
 await h.click('#restore-local-layout');
 assert.equal(h.el('#status').textContent,'Opened 4 fresh shells in the saved layout.');
 assert.deepEqual(h.el('#local-tab-list').options.map(tab=>tab.dataset.epoch),['11','12','13','14']);
 assert.equal(h.el('#terminal-panes').dataset.triple,'true');
 assert.equal(h.el('#active-pane').dataset.slot,'1');
 const observers=calls.findLast(([name])=>name==='set_local_observers')[1].observers;
 assert.deepEqual(Array.from(observers,target=>target.epoch),[13,12]);
 assert.equal(h.el('#input').disabled,false);
 assert.ok(!calls.some(([name])=>['write_shell','key_shell','text_shell','paste_shell'].includes(name)));
 assert.deepEqual(readThreePaneLayout(storage),layout);
 assert.deepEqual(readLocalShellLayout(storage),legacy);
});

test('restore creates fresh tabs with their saved shell and presentation, then selects the saved position',async()=>{
 const layout={tabs:[plainLocalTab,{shell:'jsh',presentation:'chat'}],active:0,split:false};
 const storage=layoutStorage(layout);
 const first={session:'a'.repeat(32),epoch:7,generation:1};
 const second={session:'b'.repeat(32),epoch:8,generation:2};
 const frame=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const calls=[];
 const h=harness(async(name,args)=>{
  calls.push([name,args]);
  if(name==='list_local_shells')return [];
  if(name==='open_shell')return first;
  if(name==='activate_local_tab'){
   const owner=args.targetEpoch===null?second:args.targetEpoch===first.epoch
     ?{...first,generation:3}:{...second,generation:4};
   return {Ok:{owner,frame:{...frame,session:owner.session,epoch:owner.epoch},input_allowed:true}};
  }
  if(name==='poll_shell'||name==='ack_shell')return null;
  throw Error(name);
 },undefined,undefined,undefined,storage);
 assert.equal(h.el('#restore-local-layout').hidden,false);
 assert.deepEqual(calls,[]);
 await h.click('#restore-local-layout');
 assert.equal(h.el('#restore-local-layout').hidden,true);
 assert.equal(h.el('#input').disabled,false);
 assert.equal(h.el('#input').value,'');
 assert.equal(h.el('#command').hidden,false);
 assert.match(h.el('#status').textContent,/two fresh shells/);
 assert.deepEqual(calls.filter(([name])=>name==='activate_local_tab').map(([,args])=>args.targetEpoch),[null,first.epoch]);
 assert.deepEqual(calls.filter(([name])=>name==='activate_local_tab').map(([,args])=>args.integrated),[true,undefined]);
 assert(!calls.some(([name])=>name==='write_shell'||name==='key_shell'||name==='text_shell'));
 const withGeometry={...layout,tabs:layout.tabs.map(tab=>({...tab,geometry:{rows:frame.rows.length,cols:frame.cols}})),panes:[]};
 assert.deepEqual(readLocalShellLayout(storage),withGeometry);
 h.el('#local-tab-list').options.find(tab=>tab.dataset.epoch==='8').handlers.click();await tick();
 assert.equal(h.el('#command').hidden,true);
 assert.deepEqual(readLocalShellLayout(storage),{...withGeometry,active:1});
});

test('failure to restore the jsh tab leaves the plain shell usable and the preference intact',async()=>{
 const layout={tabs:[plainLocalTab,{shell:'jsh',presentation:'chat'}],active:1,split:false};
 const storage=layoutStorage(layout);
 const calls=[];
 const h=harness(async(name)=>{
  calls.push(name);
  if(name==='list_local_shells')return [];
  if(name==='open_shell')return 7;
  if(name==='activate_local_tab')return {Err:'PTY unavailable'};
  if(name==='poll_shell')return null;
  throw Error(name);
 },undefined,undefined,undefined,storage);
 await h.click('#restore-local-layout');
 assert.match(h.el('#status').textContent,/restore incomplete.*PTY unavailable/);
 assert.equal(h.el('#input').disabled,false);
 assert.equal(h.el('#close').disabled,false);
 assert.equal(h.el('#local-tab-list').options.length,1);
 assert.equal(calls.filter(name=>name==='activate_local_tab').length,1);
 assert.deepEqual(readLocalShellLayout(storage),layout);
});

test('late geometry after a partial restore cannot replace the last complete saved layout',async()=>{
 const geometry={rows:24,cols:80};
 const layout={tabs:[plainLocalTab,plainLocalTab,plainLocalTab,plainLocalTab].map(tab=>({...tab,geometry})),active:2,split:true,panes:[1,2]};
 const storage=layoutStorage(layout),owners=new Map();
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 let next=0,revision=0,late=false;
 function frame(owner){
  const cols=owner.epoch===2&&late?79:80;
  return {...raw,session:owner.session,epoch:owner.epoch,revision:++revision,layout:revision,cols,
   rows:Array.from({length:24},()=>Array.from({length:cols},()=>({...raw.rows[0][0],text:' ',width:1})))};
 }
 const h=harness(async(name,args)=>{
  if(name==='list_local_shells')return [];
  if(name==='open_shell'||name==='activate_local_tab'){
   if(next===2)return {Err:'third PTY unavailable'};
   const epoch=++next,owner={session:String(epoch).repeat(32),epoch,generation:1};
   owners.set(epoch,owner);
   return name==='open_shell'?owner:{Ok:{owner,frame:frame(owner),input_allowed:true}};
  }
  if(name==='poll_shell'||name==='observe_local_shell')return frame(owners.get(args.epoch));
  if(name==='ack_shell')return null;
  throw Error('unexpected IPC '+name);
 },undefined,undefined,undefined,storage);
 await h.click('#restore-local-layout');
 assert.match(h.el('#status').textContent,/third PTY unavailable/);
 assert.equal(h.el('#local-tab-list').options.length,2);
 assert.equal(h.el('#input').disabled,false);
 assert.deepEqual(readLocalShellLayout(storage),layout);
 late=true;await [...h.timers.values()][0]();
 assert.match(h.el('#cursor-status').textContent,/^79 × 24/);
 assert.deepEqual(readLocalShellLayout(storage),layout);
 const save=h.el('#save-recovered-layout');
 assert.equal(save.hidden,false);assert.equal(save.disabled,false);
 const write=storage.localStorage.setItem;
 storage.localStorage.setItem=()=>{throw Error('storage full');};
 await h.click('#save-recovered-layout');
 assert.deepEqual(readLocalShellLayout(storage),layout);
 assert.equal(save.hidden,false);
 assert.match(h.el('#status').textContent,/previous layout is retained/);
 storage.localStorage.setItem=write;
 await h.click('#save-recovered-layout');
 assert.equal(save.hidden,true);
 assert.deepEqual(readLocalShellLayout(storage),{tabs:[
  {...plainLocalTab,geometry},{...plainLocalTab,geometry:{rows:24,cols:79}}],active:1,split:true,panes:[0,1]});
});

test('a lost first restore reply discovers the retained shell without launching again',async()=>{
 const storage=layoutStorage({tabs:[plainLocalTab],active:0,split:false});
 let openCalls=0;
 const h=harness(async(name)=>{
  if(name==='list_local_shells')return openCalls?[4]:[];
  if(name==='open_shell'){openCalls++;throw Error('missing open reply');}
  throw Error(name);
 },undefined,undefined,undefined,storage);
 await h.click('#restore-local-layout');
 assert.match(h.el('#status').textContent,/restore incomplete.*missing open reply/);
 assert.equal(h.el('#local-tabs').hidden,false);
 assert.equal(h.el('#restore-local-layout').hidden,true);
 assert.equal(h.el('#local-tab-list').options[0].dataset.epoch,'4');
 assert.equal(openCalls,1);
});

test('reattaching one retained shell preserves the launch and presentation of an unobserved peer',async()=>{
 const layout={tabs:[{shell:'jsh',presentation:'chat'},plainLocalTab],active:0,split:true};
 const storage=layoutStorage(layout);
 const owner={session:'b'.repeat(32),epoch:2,generation:3};
 const frame=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const h=harness(async(name,args)=>{
  switch(name){
   case 'list_local_shells':return [1,2];
   case 'attach_selected_local_shell':
    assert.equal(args.epoch,2);
    return {owner,frame:{...frame,session:owner.session,epoch:owner.epoch}};
   case 'ack_shell':case 'poll_shell':return null;
   default:throw Error(name);
  }
 },({epoch})=>({...channelHealth(epoch),command_blocks_enabled:false}),undefined,undefined,storage);
 await h.click('#manage-local');
 h.el('#local-shell-selection').value='2';
 h.el('#local-shell-selection').handlers.change();
 await h.click('#attach-selected-local');
 assert.equal(h.el('#restore-local-layout').hidden,true);
 assert.equal(h.el('#input').disabled,true);
 assert.deepEqual(readLocalShellLayout(storage),layout);
});

test('unconfirmed retirement aborts the switch and recovers only the old local shell',async()=>{
 const current={session:'a'.repeat(32),epoch:1,generation:1};
 const frame=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const pending=deferred(),calls=[];
 const h=harness(async(name,args)=>{
  calls.push(name);
  switch(name){
   case 'open_shell':return current;
   case 'list_local_shells':return [current.epoch,2];
   case 'detach_shell':return pending.promise;
   case 'attach_shell':
    assert.equal(args.target.session,current.session);assert.equal(args.target.epoch,current.epoch);
    return {owner:{...current,generation:3},frame:{...frame,session:current.session}};
   case 'poll_shell':case 'ack_shell':return null;
   default:throw Error(name);
  }
 });
 await h.click('#open');h.el('#input').value='stay with the first shell';
 await h.click('#manage-local');
 h.el('#local-shell-selection').value='2';h.el('#local-shell-selection').handlers.change();
 const switchView=h.click('#attach-selected-local');await tick();
 assert.equal(h.el('#manage-local').disabled,true);
 assert.equal(h.el('#input').disabled,true);
 assert(!calls.includes('attach_selected_local_shell'));
 pending.reject(Error('missing detach reply'));await switchView;
 assert.match(h.el('#status').textContent,/Detach unconfirmed/);
 assert(!calls.includes('attach_selected_local_shell'));
 await h.click('#attach');
 assert.equal(h.el('#input').value,'stay with the first shell');
 assert.equal(h.el('#input').disabled,true);
});

test('lost second-open response does not retry or present another shell as writable',async()=>{
 const first={session:'a'.repeat(32),epoch:3,generation:1};
 const second={session:'b'.repeat(32),epoch:4,generation:1};
 const frame=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 let uncertain=0,closed=false;
 const h=harness(async(name,args)=>{
  switch(name){
   case 'open_shell':return first;
   case 'detach_shell':case 'poll_shell':case 'ack_shell':return null;
   case 'list_local_shells':return (uncertain?[first,second]:[first]).map(owner=>owner.epoch);
   case 'open_second_local_shell':uncertain++;throw Error('reply lost');
   case 'attach_selected_local_shell':
    assert.equal(args.epoch,second.epoch);
    return {owner:{...second,generation:2},frame:{...frame,session:second.session,epoch:second.epoch}};
   case 'close_shell':assert.equal(args.epoch,second.epoch);closed=true;return {cleanup_pending:true};
   default:throw Error(name);
  }
 });
 await h.click('#open');h.el('#input').value='do not send to the other shell';
 await h.click('#detach');await h.click('#manage-local');await h.click('#open-second-local');
 assert.match(h.el('#status').textContent,/open unconfirmed/);
 assert.equal(h.el('#input').disabled,true);
 await h.click('#open-second-local');assert.equal(uncertain,1);
 await h.click('#manage-local');
 h.el('#local-shell-selection').value=String(second.epoch);h.el('#local-shell-selection').handlers.change();
 await h.click('#attach-selected-local');
 assert.equal(h.el('#input').value,'');assert.equal(h.el('#input').disabled,true);
 await h.click('#close');assert(closed);
});

test('late local roster reply cannot expose second-open controls after another view opens',async()=>{
 const list=deferred(),calls=[];
 const h=harness(async(name)=>{
  calls.push(name);
  if(name==='list_local_shells')return list.promise;
  if(name==='open_shell')return 2;
  if(name==='poll_shell')return null;
  throw Error(name);
 });
 const pending=h.click('#manage-local');
 await h.click('#open');
 list.resolve([1]);await pending;
 assert.equal(h.el('#local-shell-options').hidden,true);
 await h.click('#open-second-local');
 assert(!calls.includes('open_second_local_shell'));
});

test('confirmed local close clears only B draft; targetless reattach restores A draft',async()=>{
 const a={session:'a'.repeat(32),epoch:7,generation:1};
 const b={session:'b'.repeat(32),epoch:8,generation:1};
 const frame=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 let bCloseCalls=0;
 const h=harness(async(name,args)=>{
  switch(name){
   case 'open_shell':return a;
   case 'list_local_shells':return [a.epoch];
   case 'open_second_local_shell':assert.equal(args.epoch,a.epoch);return b;
   case 'poll_shell':case 'ack_shell':case 'detach_shell':return null;
   case 'close_shell':
     assert.equal(args.epoch,b.epoch);
     if(++bCloseCalls===1)throw Error('close not confirmed');
     return {cleanup_pending:true};
   case 'attach_shell':
     assert.equal(args,undefined);
     return {owner:{...a,generation:3},frame:{...frame,session:a.session,epoch:a.epoch}};
   default:throw Error(name);
  }
 });
 await h.click('#open');h.el('#input').value='draft only for A';
 await h.click('#detach');await h.click('#manage-local');await h.click('#open-second-local');
 h.el('#input').value='private draft only for B';
 await h.click('#close');assert.equal(h.el('#input').value,'private draft only for B');
 assert.match(h.el('#status').textContent,/Close error/);
 await h.click('#close');assert.equal(h.el('#input').value,'');
 await h.click('#attach');
 assert.equal(h.el('#input').value,'draft only for A');
 assert.equal(h.el('#input').disabled,true);
 assert.equal(bCloseCalls,2);
});

test('confirmed close clears closed Canvas, cursor and health; a failed close leaves presentation intact',async()=>{
 const fixture=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 let displayed=null,width=300,clears=0,closeCalls=0;
 const h=harness(async(name)=>{
  if(name==='open_shell')return 1;
  if(name==='poll_shell')return {...fixture,epoch:1};
  if(name==='ack_shell')return null;
  if(name==='close_shell'){
   if(++closeCalls===1)throw Error('unconfirmed');
   return {cleanup_pending:true};
  }
  throw Error(name);
 },undefined,undefined,(_canvas,frame)=>{displayed=frame;});
 Object.defineProperty(h.el('canvas'),'width',{
  get:()=>width,set:value=>{width=value;clears++;displayed=null;}
 });
 await h.click('#open');
 assert(displayed);
 const paint=displayed;
 const cursor=h.el('#cursor-status').textContent,health=h.el('#health').textContent;
 assert(cursor&&health);
 await h.click('#close');
 assert.equal(clears,0);
 assert.equal(displayed,paint);
 assert.equal(h.el('#cursor-status').textContent,cursor);
 assert.equal(h.el('#health').textContent,health);
 await h.click('#close');
 assert.equal(clears,1);
 assert.equal(displayed,null);
 assert.equal(h.el('#cursor-status').textContent,'');
 assert.equal(h.el('#health').textContent,'');
 assert.equal(h.el('#history-output').value,'');
 assert.match(h.el('#status').textContent,/Terminal closed/);
});

test('a late closed SSH snapshot cannot paint into a retained local shell that shares its epoch',async()=>{
 const fixture=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 const a={session:'a'.repeat(32),epoch:1,generation:1};
 const b={session:'b'.repeat(32),epoch:2,generation:1};
 const ssh={session:'c'.repeat(32),epoch:1,generation:1};
 const localFrame={...fixture,session:a.session,epoch:a.epoch,revision:10};
 const remoteFrame={...fixture,session:ssh.session,epoch:ssh.epoch,revision:11};
 const late=deferred(),painted=[],localAcks=[];
 let sshPolls=0,localPolls=0,readOnlyAttached=false;
 const h=harness(async(name,args)=>{
  switch(name){
   case 'open_shell':return a;
   case 'detach_shell':assert.equal(args.epoch,a.epoch);return null;
   case 'list_local_shells':return [a.epoch];
   case 'open_second_local_shell':assert.equal(args.epoch,a.epoch);return b;
   case 'close_shell':assert.equal(args.epoch,b.epoch);return {cleanup_pending:true};
   case 'attach_shell':
    readOnlyAttached=true;return {owner:{...a,generation:3},frame:localFrame};
   case 'poll_shell':if(readOnlyAttached)localPolls++;return null;
   case 'ack_shell':localAcks.push(args.revision);return null;
   case 'ssh_terminal': {
    const {action}=args;
    if(action.operation==='open')return ssh;
    if(action.command.kind==='poll')return ++sshPolls===1?null:late.promise;
    if(action.command.kind==='status')return {epoch:1,drained:false,exited:false,write_error:null,dropped_reply_bytes:'0',unsupported:'0'};
    if(action.command.kind==='close')return {cleanup_pending:true};
    throw Error(action.command.kind);
   }
   default:throw Error(name);
  }
 },undefined,undefined,(_canvas,frame)=>painted.push(frame.session));
 await h.click('#open');await h.click('#detach');await h.click('#manage-local');
 await h.click('#open-second-local');await h.click('#close');
 h.el('#ssh-terminal-host').value='fixture';
 await h.submitSsh();
 const inFlight=[...h.timers.values()][0]();
 await tick();assert.equal(sshPolls,2);
 await h.click('#close');await h.click('#attach');
 const freshPolls=localPolls;
 late.resolve({kind:'snapshot',frame:remoteFrame});await inFlight;
 assert.deepEqual(painted,[a.session]);
 assert.deepEqual(localAcks,[localFrame.revision]);
 assert.equal(freshPolls,1);
 assert.equal(h.el('#input').disabled,true);
 assert.match(h.el('#status').textContent,/view-only/);
});

test('public restore opens three fresh tabs, confirms each size, preserves split pair and active tab',async()=>{
 const sizes=[{rows:17,cols:71},{rows:12,cols:83},{rows:19,cols:95}];
 const layout={tabs:sizes.map(geometry=>({...plainLocalTab,geometry})),active:1,split:true,panes:[0,1]};
 const storage=layoutStorage(layout),calls=[],owners=new Map(),physical=new Map();let next=100,revision=1;
 const raw=JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8'));
 function frame(owner){
  const size=physical.get(owner.epoch);
  return {...raw,session:owner.session,epoch:owner.epoch,revision:++revision,layout:revision,cols:size.cols,
   rows:Array.from({length:size.rows},()=>Array.from({length:size.cols},()=>({...raw.rows[0][0],text:' ',width:1})))};
 }
 const h=harness(async(name,args)=>{
  calls.push([name,structuredClone(args)]);
  if(name==='list_local_shells')return [];
  if(name==='open_shell'){
   const owner={session:'a'.repeat(32),epoch:++next,generation:1};owners.set(owner.epoch,owner);physical.set(owner.epoch,{rows:24,cols:80});return owner;
  }
  if(name==='activate_local_tab'){
   let owner;
   if(args.targetEpoch===null){owner={session:String.fromCharCode(97+owners.size).repeat(32),epoch:++next,generation:1};physical.set(owner.epoch,{rows:24,cols:80});}
   else owner={...owners.get(args.targetEpoch),generation:owners.get(args.targetEpoch).generation+1};
   owners.set(owner.epoch,owner);return {Ok:{owner,frame:frame(owner),input_allowed:true}};
  }
  if(name==='resize_shell'){physical.set(args.epoch,{rows:args.rows,cols:args.cols});return null;}
  if(name==='poll_shell')return frame(owners.get(args.epoch));
  if(name==='observe_local_shell')return frame(owners.get(args.epoch));
  if(name==='ack_shell')return null;
  throw Error('unexpected IPC '+name);
 },undefined,undefined,undefined,storage);
 assert.deepEqual(readLocalShellLayout(storage),layout);
 assert.equal(h.el('#restore-local-layout').hidden,false);
 await h.click('#restore-local-layout');
 assert.equal(h.el('#status').textContent,'Opened 3 fresh shells in the saved layout.');
 assert.equal(h.el('#local-tab-list').options.length,3);
 assert.equal(h.el('#cols').value,'83');assert.equal(h.el('#rows').value,'12');
 assert.equal(h.el('#terminal-panes').dataset.split,'right');
 assert.deepEqual(calls.filter(([name])=>name==='resize_shell').map(([,args])=>({rows:args.rows,cols:args.cols})),sizes);
 assert.deepEqual(calls.filter(([name])=>name==='activate_local_tab').map(([,args])=>args.targetEpoch),[null,null,102]);
 assert(!calls.some(([name])=>['write_shell','key_shell','text_shell','paste_shell'].includes(name)));
 await [...h.timers.values()][0]();
 assert.equal(h.el('#peer-canvas').dataset.geometry,'71x17');
 assert.deepEqual(readLocalShellLayout(storage),layout);
 const tabList=h.el('#local-tab-list'),tabButtons=tabList.options;
 const [,before,targetButton]=tabButtons;
 assert.equal(before.attributes.get('aria-current'),'true');
 assert.equal(targetButton.attributes.has('aria-current'),false);
 targetButton.handlers.click();await tick();
 assert.equal(tabList.options,tabButtons,'tab activation must not replace the native buttons when the roster is unchanged');
 assert.equal(before.attributes.has('aria-current'),false);
 assert.equal(targetButton.attributes.get('aria-current'),'true');
 assert.equal(targetButton.disabled,false);
 await [...h.timers.values()][0]();
 assert.equal(h.el('#peer-canvas').dataset.geometry,'71x17');
 assert.equal(h.el('#cols').value,'95');assert.equal(h.el('#rows').value,'19');
 const saved=readLocalShellLayout(storage);assert.equal(saved.active,2);assert.deepEqual(saved.panes,[0,2]);
 assert.deepEqual(saved.tabs.map(tab=>tab.geometry),sizes);
});


test('ordinary SSH link loss retains recovery and rejects an unowned late commit',async()=>{
 const old={session:'a'.repeat(32),epoch:1,generation:1},next={...old,generation:2};
 const frame={...JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8')),session:old.session,epoch:1};
 let lost=false;const sent=[];
 const h=harness(async(name,{action})=>{
  assert.equal(name,'ssh_terminal');
  if(action.operation==='open')return old;
  const c=action.command;
  if(c.kind==='poll'){if(lost)throw Error('owned SSH link lost');return null;}
  if(c.kind==='status')return {epoch:1,exited:false,drained:false,write_error:null,unsupported:'0',dropped_reply_bytes:'0'};
  if(c.kind==='recover'){lost=false;return {owner:next,frame,input_allowed:true};}
  if(c.kind==='text'){sent.push([c.owner,c.text]);return null;}
  if(c.kind==='ack')return null;
  throw Error(c.kind);
 });
 h.el('#ssh-terminal-host').value='fixture@127.0.0.1';await h.submitSsh();
 const ime=h.el('#ime-input');
 lost=true;await [...h.timers.values()][0]();
 assert.equal(h.el('#input').disabled,true);assert.equal(h.el('#terminal-preedit').hidden,true);
 await h.click('#recover');assert.equal(h.el('#input').disabled,false);
 ime.handlers.compositionend({data:'你好'});ime.handlers.input({data:'你好',inputType:'insertText'});await tick();assert.deepEqual(sent,[]);
 ime.handlers.compositionstart();ime.handlers.compositionupdate({data:'世界'});ime.handlers.compositionend({data:'世界'});ime.handlers.input({data:'世界',inputType:'insertText'});await tick();
 assert.equal(sent.length,1);assert.equal(sent[0][0].generation,2);assert.equal(sent[0][1],'世界');
});

test('composing SSH link loss uses one native reload ticket and fresh granted generation without launching',async()=>{
 const old={session:'a'.repeat(32),epoch:1,generation:1},next={...old,generation:3};
 const frame={...JSON.parse(fs.readFileSync(new URL('../web/demo-frame.json',import.meta.url),'utf8')),session:old.session,epoch:1};
 const items=new Map(),sessionStorage={getItem:k=>items.get(k),setItem:(k,v)=>items.set(k,v),removeItem:k=>items.delete(k)};
 let lost=false,reloads=0,view,prepared=0,claimed=0;const sent=[];
 const reply=async(name,{action})=>{
  assert.equal(name,'ssh_terminal');
  if(action.operation==='open')return old;
  if(action.operation==='prepare_reload'){prepared++;assert.deepEqual(action.owner,old);view=action.view;return {ticket:'f'.repeat(32)};}
  if(action.operation==='claim_reload'){claimed++;assert.equal(items.size,0);return {owner:next,frame,input_allowed:true,view};}
  const c=action.command;
  if(c.kind==='poll'){if(lost)throw Error('owned SSH link lost');return null;}
  if(c.kind==='status')return {epoch:1,exited:false,drained:false,write_error:null,unsupported:'0',dropped_reply_bytes:'0'};
  if(c.kind==='text'){sent.push([c.owner,c.text]);return null;}
  if(c.kind==='ack')return null;
  throw Error('unexpected '+c.kind);
 };
 const h=harness(reply,undefined,TextEncoder,()=>{},{},undefined,{sessionStorage,location:{reload(){reloads++;}}});
 h.el('#ssh-terminal-host').value='private';await h.submitSsh();h.el('#input').value='kept draft';
 const oldField=h.el('#ime-input');oldField.handlers.compositionstart();oldField.handlers.compositionupdate({data:'你好'});
 lost=true;await [...h.timers.values()][0]();await h.click('#recover');
 assert.equal(prepared,1);assert.equal(reloads,1);assert.equal(h.el('#input').disabled,true);
 oldField.handlers.compositionend({data:'你好'});oldField.handlers.input({data:'你好',inputType:'insertText'});await tick();assert.deepEqual(sent,[]);
 lost=false;
 const restored=harness(reply,undefined,TextEncoder,()=>{},{},undefined,{sessionStorage});await tick();await tick();
 assert.equal(claimed,1);assert.equal(restored.el('#input').value,'kept draft');assert.equal(restored.el('#input').disabled,false);
 const field=restored.el('#ime-input');field.handlers.compositionstart();field.handlers.compositionend({data:'世界'});await tick();
 assert.equal(sent.length,1);assert.equal(sent[0][0].generation,3);assert.equal(sent[0][1],'世界');
});

test('expired SSH claim recovers only retained local metadata, keeps input closed, and never launches',async()=>{
 const local={session:'b'.repeat(32),epoch:4,generation:2};
 const view={version:1,tabs:[{owner:local,shell:'plain',paneId:null,geometry:{cols:81,rows:21},draft:'LOCAL UNSENT'}],splitEnabled:false,splitEpochs:[],detachedLocal:local,remoteDraft:'REMOTE MUST NOT ENTER LOCAL'};
 const items=new Map([['aether.ssh-writer-reload','f'.repeat(32)]]),sessionStorage={getItem:k=>items.get(k),removeItem:k=>items.delete(k)};
 const calls=[];
 const h=harness(async(name,args)=>{
  calls.push([name,args]);
  if(name==='ssh_terminal'&&args.action.operation==='claim_reload')throw Error('expired');
  if(name==='list_local_shells')return [4];
  throw Error('unexpected launch or input '+name);
 },undefined,TextEncoder,()=>{},{},undefined,{sessionStorage,recoverReloadLayout:()=>({view})});
 await tick();await tick();
 assert.equal(h.el('#restore-local-layout').hidden,true);assert.equal(h.el('#input').disabled,true);assert.equal(h.el('#input').value,'');
 assert.deepEqual(h.el('#local-tab-list').options.map(tab=>tab.dataset.epoch),['4']);
 assert.match(h.el('#status').textContent,/view-only/);assert.equal(items.size,0);
 assert.equal(calls.filter(([name])=>name==='open_shell'||name==='open_second_local_shell'||name==='activate_local_tab').length,0);
});

test('malformed or missing SSH ticket can consume orphaned reader metadata without claiming a writer',async()=>{
 for(const ticket of ['malformed',null]){
  const items=new Map(ticket===null?[]:[['aether.ssh-writer-reload',ticket]]),sessionStorage={getItem:k=>items.get(k),removeItem:k=>items.delete(k)};
  let claims=0;const view={version:1,tabs:[],splitEnabled:false,splitEpochs:[],detachedLocal:null,remoteDraft:''};
  const h=harness(async(name,args)=>{if(name==='list_local_shells')return [];if(name==='ssh_terminal'){claims++;throw Error('writer claim forbidden');}throw Error(name);},undefined,TextEncoder,()=>{},{},undefined,{sessionStorage,recoverReloadLayout:()=>({view})});
  await tick();await tick();assert.equal(claims,0);assert.equal(h.el('#input').disabled,true);assert.match(h.el('#status').textContent,/not completed/);
 }
});

test('a delayed reader metadata reply cannot replace a newly opened local pane',async()=>{
 const pending=deferred();const local={session:'b'.repeat(32),epoch:4,generation:2};
 const view={version:1,tabs:[{owner:local,shell:'plain',paneId:null,geometry:null,draft:'STALE'}],splitEnabled:false,splitEpochs:[],detachedLocal:null,remoteDraft:''};
 const h=harness(async name=>{if(name==='open_shell')return 42;if(name==='poll_shell')return null;throw Error(name);},undefined,TextEncoder,()=>{},{},undefined,{recoverReloadLayout:()=>pending.promise});
 await h.click('#open');h.el('#input').value='FRESH';pending.resolve({view});await tick();await tick();
 assert.equal(h.el('#input').disabled,false);assert.equal(h.el('#input').value,'FRESH');
 assert.deepEqual(h.el('#local-tab-list').options.map(tab=>tab.dataset.epoch),['42']);
});

test('failed handoff without metadata never launches a saved layout while retained state exists or is unknown',async()=>{
 for(const unavailable of [false,true]){
  const items=new Map([['aether.ssh-writer-reload','invalid']]),sessionStorage={getItem:k=>items.get(k),removeItem:k=>items.delete(k)};
  const storage=layoutStorage({tabs:[plainLocalTab],active:0,split:false});let opens=0;
  const h=harness(async name=>{if(name==='list_local_shells'){if(unavailable)throw Error('unavailable');return [4];}if(name==='open_shell'){opens++;return 5;}throw Error(name);},undefined,TextEncoder,()=>{},storage,undefined,{sessionStorage});
  await tick();await tick();assert.equal(h.el('#restore-local-layout').hidden,true);assert.equal(h.el('#restore-local-layout').disabled,true);
  await h.click('#restore-local-layout');assert.equal(opens,0);assert.equal(h.el('#input').disabled,true);
 }
});
