import {installFilePreviewPanel} from './file-preview-panel.js';
import {installPasteReview} from './paste.js';
import {resizeQueue,cursorTarget} from './viewport.js';
import {installHistory} from './history.js';
import {acceptFull,acceptUpdate,paint} from './frames.js'; import {installIme} from './ime.js';
const canvas=document.querySelector('canvas'),status=document.querySelector('#status'),button=document.querySelector('#demo');
const open=document.querySelector('#open'),attach=document.querySelector('#attach'),detach=document.querySelector('#detach'),close=document.querySelector('#close'),input=document.querySelector('#input'),send=document.querySelector('#send');
const sshForm=document.querySelector('#ssh-terminal-form'),sshHost=document.querySelector('#ssh-terminal-host'),openSsh=document.querySelector('#open-ssh');
let state=null,epoch=null,owner=null,timer=null,polling=null,generation=0,transport=0,recovery=null;
let sshSession=null,sshInputSuspended=false;
const filePreview=installFilePreviewPanel(document,{getFrame:()=>state,getEpoch:()=>epoch,getSession:()=>owner?.session});
const imeInput=document.querySelector('#ime-input');
const ime=installIme(imeInput,{getEpoch:()=>epoch!==null&&!input.disabled?epoch:null,commit:(text,owner)=>{if(owner!==epoch)return;if(text.length>4096||new TextEncoder().encode(text).length>4096){status.textContent='Committed text exceeds 4096-byte limit; nothing sent.';return;}enqueueLogical('text_shell',{text});}});
canvas.addEventListener('click',event=>{if(!event.ctrlKey)imeInput.focus();});
const resize=document.querySelector('#resize');
const invoke=globalThis.__TAURI__?.core?.invoke;
const sshCommands={poll_shell:'poll',ack_shell:'ack',shell_status:'status',history_shell:'history',recover_shell:'recover',detach_shell:'detach',close_shell:'close',write_shell:'write',key_shell:'key',text_shell:'text',paste_shell:'paste',resize_shell:'resize'};
function setOpenDisabled(disabled) {
  open.disabled=openSsh.disabled=sshHost.disabled=disabled||!invoke;
}
function invokeAttached(command,{epoch:target,...args},attachment=owner) {
  if(!attachment||attachment.epoch!==target) return Promise.reject(new Error('terminal session changed'));
  if(attachment.session===sshSession){
    const kind=sshCommands[command];
    if(!kind)return Promise.reject(new Error('Unknown SSH terminal command'));
    return invoke('ssh_terminal',{action:{operation:'request',command:{kind,owner:attachment,...args}}});
  }
  return invoke(command,{owner:attachment,...args});
}
function acceptOwner(attachment) {
  const valid = value=>Number.isInteger(value)&&value>0&&value<=0xffffffff;
  if(!attachment||typeof attachment.session!=='string'||!/^[a-f0-9]{32}$/.test(attachment.session)||
      !valid(attachment.epoch)||!valid(attachment.generation)) throw new Error('invalid terminal attachment');
  return attachment;
}
const pasteReview=installPasteReview({panel:document.querySelector('#paste-review'),preview:document.querySelector('#paste-preview'),escaped:document.querySelector('#paste-escaped'),confirm:document.querySelector('#paste-confirm'),cancel:document.querySelector('#paste-cancel'),getEpoch:()=>epoch!==null&&!input.disabled?epoch:null,send:(text,owner)=>{if(owner===epoch)enqueueLogical('paste_shell',{text});},onError:message=>{status.textContent=message;}});
const history=installHistory({older:document.querySelector('#history-older'),newer:document.querySelector('#history-newer'),refresh:document.querySelector('#history-refresh'),output:document.querySelector('#history-output'),label:document.querySelector('#history-label'),invoke:invokeAttached,getEpoch:()=>epoch});
sshForm.hidden=!invoke;
button.textContent=invoke?'Next Rust frame':'Load verified Rust fixture';setOpenDisabled(false);attach.disabled=!invoke;
button.addEventListener('click',async()=>{
  button.disabled=true;setOpenDisabled(true);attach.disabled=true;const currentGeneration=++generation;
  try {
    let frame;
    if(invoke) frame=await invoke('demo_frame');
    else {
      const res=await fetch('./demo-frame.json');if(!res.ok)throw new Error('Generate demo-frame.json with the Rust bridge first');
      const bytes=await res.arrayBuffer();if(bytes.byteLength>1048576)throw new Error('frame byte limit');
      frame=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(bytes));
    }
    if(currentGeneration!==generation)return;
    const next=acceptFull(state,frame);paint(canvas,next);state=next;describeCursor();
    if(invoke) await invoke('acknowledge',{session:state.session,epoch:state.epoch,revision:state.revision});
    status.textContent=`Rust frame ${state.revision} accepted. No PTY attached.`;
  }catch(e){if(currentGeneration!==generation)return;status.textContent=`Stopped: ${String(e?.message??e)}. Restart the app to reset.`;}
  finally {if(currentGeneration===generation){button.disabled=!invoke&&state!==null;setOpenDisabled(false);attach.disabled=!invoke;}}
});
async function poll(){
  if(epoch===null||polling?.epoch===epoch)return;const current=epoch,attachment=owner,ticket={epoch,transport};polling=ticket;
  try{const frame=await invokeAttached('poll_shell',{epoch:current},attachment);if(epoch!==current||ticket.transport!==transport)return;
    if(frame){const next=acceptUpdate(state,frame,attachment);paint(canvas,next);state=next;describeCursor();await invokeAttached('ack_shell',{epoch:current,revision:next.revision},attachment);}
    if(epoch!==current||ticket.transport!==transport)return;
    const info=await invokeAttached('shell_status',{epoch:current},attachment);if(epoch!==current||ticket.transport!==transport)return;
    if(!info||info.epoch!==current||ticket.transport!==transport||typeof info.exited!=='boolean'||typeof info.drained!=='boolean'||(info.write_error!==null&&typeof info.write_error!=='string')||typeof info.dropped_reply_bytes!=='string'||!/^\d{1,20}$/.test(info.dropped_reply_bytes)||typeof info.unsupported!=='string'||!/^\d{1,20}$/.test(info.unsupported))throw new Error('invalid session status');
    document.querySelector('#health').textContent=`Unsupported sequences: ${info.unsupported}. Dropped reply bytes: ${info.dropped_reply_bytes}. Shell ${info.exited?'exited; output remains visible':'running'}.${info.write_error?' Input disabled: '+info.write_error:''}`;
    // Continue bounded polling after child exit so trailing PTY bytes are drained.
    if(info.exited||info.write_error){pasteReview.reset();keyQueue.locked=true;input.disabled=send.disabled=resize.disabled=true;}
    if(info.drained)clearInterval(timer);

  }catch(e){
    if(epoch!==current||ticket.transport!==transport)return;
    const isSsh=attachment.session===sshSession;
    sshInputSuspended=isSsh&&!keyQueue.locked&&!keyQueue.pending&&!input.disabled;
    pasteReview.reset();keyQueue.locked=true;clearInterval(timer);
    status.textContent=isSsh?`SSH terminal paused: ${String(e?.message??e)}. Use Recover output snapshot.`:`Shell stopped: ${String(e?.message??e)}. Close/reopen before continuing.`;
    input.disabled=send.disabled=resize.disabled=true;
  }
  finally{if(polling===ticket)polling=null;}
}
async function openShell(destination=null){
  if(epoch!==null||sshSession)return;
  const attempt=++generation;transport++;sshInputSuspended=false;
  filePreview.dismiss();pasteReview.reset();history.reset();setOpenDisabled(true);attach.disabled=button.disabled=true;
  if(destination!==null)status.textContent=`Opening SSH terminal on ${destination}…`;
  try{
    const result=destination===null?await invoke('open_shell'):await invoke('ssh_terminal',{action:{operation:'open',destination}});
    if(generation!==attempt)return;
    owner=acceptOwner(result);sshSession=destination===null?null:owner.session;
    epoch=owner.epoch;keyQueue={chain:Promise.resolve(),pending:0,locked:false};state=null;
    resize.disabled=close.disabled=detach.disabled=input.disabled=send.disabled=false;
    status.textContent=destination===null?'Experimental /bin/sh; TERM=dumb. Focus terminal for experimental keyboard input; no native GUI/IME acceptance.':`SSH shell connected to ${destination}.`;
    timer=setInterval(poll,50);await poll();
  }catch(e){if(generation===attempt){status.textContent=String(e?.message??e);setOpenDisabled(false);attach.disabled=button.disabled=false;}}
}
open.addEventListener('click',()=>openShell());
sshForm.addEventListener('submit',event=>{
  event.preventDefault();
  const destination=sshHost.value.trim();
  if(destination&&!openSsh.disabled)return openShell(destination);
});
attach.addEventListener('click',async()=>{
  if(epoch!==null||attach.disabled)return;
  const attempt=++generation;filePreview.dismiss();pasteReview.reset();ime.reset();history.reset();
  setOpenDisabled(true);attach.disabled=button.disabled=true;
  try {
    const result=sshSession===null?await invoke('attach_shell'):await invoke('ssh_terminal',{action:{operation:'attach'}});
    if(generation!==attempt)return;
    if(!result){sshSession=null;status.textContent='No existing shell session.';return;}
    const nextOwner=acceptOwner(result.owner);
    const next=acceptFull(null,result.frame,nextOwner);
    owner=nextOwner;epoch=owner.epoch;keyQueue={chain:Promise.resolve(),pending:0,locked:true};
    input.disabled=send.disabled=resize.disabled=true;close.disabled=detach.disabled=false;
    paint(canvas,next);state=next;describeCursor();
    await invokeAttached('ack_shell',{epoch:nextOwner.epoch,revision:next.revision},nextOwner);
    if(owner!==nextOwner)return;
    status.textContent=sshSession===null?'Reconnected to the existing shell in view-only mode.':'Reconnected to the SSH shell in view-only mode.';
    timer=setInterval(poll,50);await poll();
  }catch(e){if(generation===attempt)status.textContent=String(e?.message??e);}
  finally{if(generation===attempt&&epoch===null){setOpenDisabled(sshSession!==null);attach.disabled=false;button.disabled=sshSession!==null;}}
});
detach.addEventListener('click',async()=>{
  if(!owner||detach.disabled||recovery?.epoch===epoch)return;
  const attempt=++generation,previous=owner;owner=null;epoch=null;state=null;transport++;
  clearInterval(timer);sshInputSuspended=false;filePreview.dismiss();pasteReview.reset();ime.reset();history.reset();
  resize.disabled=close.disabled=detach.disabled=input.disabled=send.disabled=true;
  canvas.width=canvas.width;
  document.querySelector('#cursor-status').textContent='';
  document.querySelector('#health').textContent='';
  try {
    await invokeAttached('detach_shell',{epoch:previous.epoch},previous);
    status.textContent='View detached. Reconnect to see the running shell.';
  }catch(e){
    status.textContent=`Detach unconfirmed: ${String(e?.message??e)}. Reconnect to recover the view.`;
  }finally{
    if(generation===attempt)attach.disabled=false;
  }
});
close.addEventListener('click',async()=>{
  generation++;filePreview.dismiss();pasteReview.reset();ime.reset();history.reset();clearInterval(timer);const old=epoch,oldOwner=owner;epoch=null;owner=null;resize.disabled=close.disabled=detach.disabled=input.disabled=send.disabled=true;
  try{const result=await invokeAttached('close_shell',{epoch:old},oldOwner);status.textContent=result?.cleanup_pending===false?'Terminal closed; direct child reaped. Descendant containment is not provided.':'Terminal closed; direct-child cleanup handed to bounded supervisor and may still be pending. Descendant containment is not provided.';state=null;sshSession=null;sshInputSuspended=false;setOpenDisabled(false);attach.disabled=button.disabled=false;}
  catch(e){epoch=old;owner=oldOwner;close.disabled=detach.disabled=false;status.textContent=`Close error: ${String(e?.message??e)}. Retry Close to finish cleanup.`;}
});
document.querySelector('#command').addEventListener('submit',async event=>{
  event.preventDefault();if(epoch===null)return;
  // Reserve one byte for the appended newline before creating an encoded copy.
  if(input.value.length>4095){status.textContent='Input exceeds 4096-byte bound';return;}
  const bytes=new TextEncoder().encode(input.value+'\n');if(bytes.length>4096){status.textContent='Input exceeds 4096-byte bound';return;}
  send.disabled=true;input.disabled=true;const current=epoch,attachment=owner;let at=0;let failed=false;
  try{await keyQueue.chain;if(epoch!==current||keyQueue.locked)throw new Error('input unavailable');for(let attempts=0;at<bytes.length&&attempts<128;attempts++){
    if(epoch!==current)throw new Error('session changed');
    if(keyQueue.locked)throw new Error('input became unavailable');
    const n=await invokeAttached('write_shell',{epoch:current,bytes:Array.from(bytes.subarray(at))},attachment);
    // A final old-epoch acknowledgement must not clear a reopened session's draft.
    if(epoch!==current)return;
    if(!Number.isInteger(n)||n<0||n>bytes.length-at)throw new Error('invalid write count');at+=n;
    if(n===0)await new Promise(resolve=>setTimeout(resolve,10));
  }if(at!==bytes.length)throw new Error('write blocked; do not resend automatically');input.value='';}
  catch(e){failed=true;if(epoch===current){pasteReview.reset();keyQueue.locked=true;status.textContent=`${at} bytes accepted: ${String(e?.message??e)}. Input locked; close/reopen before retrying.`;}}
  finally{if(epoch===current&&!failed&&!keyQueue.locked){send.disabled=false;input.disabled=false;input.focus();}}
});

const colsInput=document.querySelector('#cols'),rowsInput=document.querySelector('#rows');
colsInput.value ||= '100';rowsInput.value ||= '24';
const sizes=resizeQueue({getEpoch:()=>owner,invoke:(command,{epoch:attachment,...args})=>invokeAttached(command,{epoch:attachment.epoch,...args},attachment),onDone:async()=>{await poll();},onError:e=>{status.textContent=String(e?.message??e);}});
resize.addEventListener('click',async()=>{if(epoch===null||input.disabled)return;try{await sizes.request(Number(rowsInput.value),Number(colsInput.value));}catch(e){status.textContent=String(e?.message??e);}});
function describeCursor(){if(state)document.querySelector('#cursor-status').textContent=`${state.cols} × ${state.rows.length} cells · cursor row ${state.cursor_row+1}, column ${state.cursor_col+1} · ${state.cursor_shape}${state.cursor_visible?'':' (hidden)'}${state.pending_wrap?' · wrap pending':''}`;}
document.querySelector('#reveal-cursor').addEventListener('click',()=>{if(!state)return;const viewport=document.querySelector('.viewport');viewport.scrollTo(cursorTarget(state,canvas,viewport));});


// Browser supplies logical events only; Rust chooses all terminal wire sequences.
let keyQueue={chain:Promise.resolve(),pending:0,locked:false};
function enqueueLogical(command,payload){
  if(epoch===null||input.disabled)return;
  const queue=keyQueue;
  if(queue.pending>=128){pasteReview.reset();queue.locked=true;status.textContent='Keyboard queue full; input locked. Close/reopen before retrying.';input.disabled=send.disabled=true;return;}
  const current=epoch,attachment=owner;queue.pending++;
  queue.chain=queue.chain.then(async()=>{
    if(epoch!==current||queue.locked)return;
    try{await invokeAttached(command,{epoch:current,...payload},attachment);}
    catch(e){if(epoch===current){pasteReview.reset();queue.locked=true;input.disabled=send.disabled=true;status.textContent=`Input not accepted: ${String(e?.message??e)}. Close/reopen before retrying.`;}}
  }).finally(()=>{queue.pending--;});
}
function keydown(event){
  // Respect an earlier control or capture handler that already owns this key.
  if(event.defaultPrevented)return;
  if(epoch===null||input.disabled||ime.isComposing()||event.isComposing||event.keyCode===229||event.getModifierState?.('AltGraph')||event.metaKey||event.key==='Dead'||event.key==='Process')return;
  if(pasteReview.dismissFromOrigin(event))return;
  // Leave browser paste shortcut alone; the paste event carries clipboard text.
  if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='v')return;
  // Linux browser paste must reach the same review path as Ctrl+V.
  // Preserve plain Insert and modified Insert combinations for terminal apps.
  if(event.key==='Insert'&&event.shiftKey&&!event.ctrlKey&&!event.altKey)return;
  const named=/^(Arrow(Up|Down|Left|Right)|Home|End|Insert|Delete|PageUp|PageDown|F([1-9]|1[0-2])|Enter|Escape|Backspace|Tab)$/;
  if(!named.test(event.key)&&[...event.key].length!==1)return;
  if(event.currentTarget===imeInput&&!named.test(event.key)&&!event.ctrlKey&&!event.altKey)return;
  event.preventDefault();enqueueLogical('key_shell',{name:event.key,shift:event.shiftKey,alt:event.altKey,ctrl:event.ctrlKey});
}
canvas.addEventListener('keydown',keydown);imeInput.addEventListener('keydown',keydown);
function paste(event){
  if(epoch===null||input.disabled)return;
  event.preventDefault();const text=event.clipboardData?.getData('text/plain')||'';
  pasteReview.offer(text,event.currentTarget);
}
canvas.addEventListener('paste',paste);imeInput.addEventListener('paste',paste);

document.querySelector('#copy-visible').addEventListener('click',()=>{
  if(!state)return;
  history.reset();
  const output=document.querySelector('#history-output');
  // Match Rust archive copy: strip ASCII blank padding, not displayed
  // Unicode spaces such as NBSP or the wide ideographic space.
  output.value=state.rows.map(row=>row.filter(cell=>cell.width!==0).map(cell=>cell.text).join('').replace(/ +$/,'')).join('\n');
  document.querySelector('#history-label').textContent=`Visible grid snapshot ${state.revision}. Select and copy normally; visual rows are separated by newlines.`;
});

document.querySelector('#recover').addEventListener('click',async()=>{
  if(epoch===null||recovery?.epoch===epoch)return;
  const current=epoch,attachment=owner,ticket=++transport,operation={epoch:current};recovery=operation;detach.disabled=true;clearInterval(timer);polling=null;
  try {
    const result=await invokeAttached('recover_shell',{epoch:current},attachment);
    if(epoch!==current||transport!==ticket)return;
    const nextOwner=acceptOwner(result.owner);
    if(nextOwner.session!==attachment.session||nextOwner.epoch!==current||
        nextOwner.generation<=attachment.generation) throw new Error('foreign terminal recovery');
    const next=acceptFull(null,result.frame,nextOwner);
    if(keyQueue.pending||(!keyQueue.locked&&input.disabled)){
      pasteReview.reset();keyQueue.locked=true;input.disabled=send.disabled=true;
    }
    owner=nextOwner;
    paint(canvas,next);state=next;describeCursor();
    await invokeAttached('ack_shell',{epoch:current,revision:next.revision});
    if(epoch!==current||transport!==ticket)return;
    if(nextOwner.session===sshSession){
      if(sshInputSuspended&&result.input_allowed===true&&!keyQueue.pending){keyQueue.locked=false;input.disabled=send.disabled=resize.disabled=false;}
      sshInputSuspended=false;status.textContent='SSH output snapshot recovered.';
    }else{status.textContent='Output snapshot recovered. Existing input safety locks remain unchanged.';}
    timer=setInterval(poll,50);await poll();
  }catch(e){if(epoch===current&&transport===ticket)status.textContent=`Recovery failed: ${String(e?.message??e)}. Retry recovery or close.`;}
  finally{if(recovery===operation){recovery=null;if(epoch===current)detach.disabled=false;}}
});
