import {installPasteReview} from './paste.js';
import {resizeQueue,cursorTarget} from './viewport.js';
import {installHistory} from './history.js';
import {acceptFull,paint} from './frames.js'; import {installIme} from './ime.js';
const canvas=document.querySelector('canvas'),status=document.querySelector('#status'),button=document.querySelector('#demo');
const open=document.querySelector('#open'),close=document.querySelector('#close'),input=document.querySelector('#input'),send=document.querySelector('#send');
let state=null,epoch=null,timer=null,polling=null,generation=0,transport=0,recovery=null;
const imeInput=document.querySelector('#ime-input');
const ime=installIme(imeInput,{getEpoch:()=>epoch!==null&&!input.disabled?epoch:null,commit:(text,owner)=>{if(owner!==epoch)return;if(text.length>4096||new TextEncoder().encode(text).length>4096){status.textContent='Committed text exceeds 4096-byte limit; nothing sent.';return;}enqueueLogical('text_shell',{text});}});
canvas.addEventListener('click',()=>imeInput.focus());
const resize=document.querySelector('#resize');
const invoke=globalThis.__TAURI__?.core?.invoke;
const pasteReview=installPasteReview({panel:document.querySelector('#paste-review'),preview:document.querySelector('#paste-preview'),escaped:document.querySelector('#paste-escaped'),confirm:document.querySelector('#paste-confirm'),cancel:document.querySelector('#paste-cancel'),getEpoch:()=>epoch!==null&&!input.disabled?epoch:null,send:(text,owner)=>{if(owner===epoch)enqueueLogical('paste_shell',{text});},onError:message=>{status.textContent=message;}});
const history=installHistory({older:document.querySelector('#history-older'),newer:document.querySelector('#history-newer'),refresh:document.querySelector('#history-refresh'),output:document.querySelector('#history-output'),label:document.querySelector('#history-label'),invoke,getEpoch:()=>epoch});
button.textContent=invoke?'Next Rust frame':'Load verified Rust fixture';open.disabled=!invoke;
button.addEventListener('click',async()=>{
  button.disabled=true;open.disabled=true;const currentGeneration=++generation;
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
  finally {if(currentGeneration===generation){button.disabled=!invoke&&state!==null;open.disabled=!invoke;}}
});
async function poll(){
  if(epoch===null||polling?.epoch===epoch)return;const current=epoch,ticket={epoch,transport};polling=ticket;
  try{const frame=await invoke('poll_shell',{epoch:current});if(epoch!==current||ticket.transport!==transport)return;
    if(frame){const next=acceptFull(state,frame,{session:'pty-demo',epoch:current});paint(canvas,next);state=next;describeCursor();await invoke('ack_shell',{epoch:current,revision:next.revision});}
    if(epoch!==current||ticket.transport!==transport)return;
    const info=await invoke('shell_status',{epoch:current});if(epoch!==current||ticket.transport!==transport)return;
    if(!info||info.epoch!==current||ticket.transport!==transport||typeof info.exited!=='boolean'||typeof info.drained!=='boolean'||(info.write_error!==null&&typeof info.write_error!=='string')||typeof info.dropped_reply_bytes!=='string'||!/^\d{1,20}$/.test(info.dropped_reply_bytes)||typeof info.unsupported!=='string'||!/^\d{1,20}$/.test(info.unsupported))throw new Error('invalid session status');
    document.querySelector('#health').textContent=`Unsupported sequences: ${info.unsupported}. Dropped reply bytes: ${info.dropped_reply_bytes}. Shell ${info.exited?'exited; output remains visible':'running'}.${info.write_error?' Input disabled: '+info.write_error:''}`;
    // Continue bounded polling after child exit so trailing PTY bytes are drained.
    if(info.exited||info.write_error){pasteReview.reset();keyQueue.locked=true;input.disabled=send.disabled=resize.disabled=true;}
    if(info.drained)clearInterval(timer);

  }catch(e){if(epoch!==current||ticket.transport!==transport)return;pasteReview.reset();keyQueue.locked=true;clearInterval(timer);status.textContent=`Shell stopped: ${String(e?.message??e)}. Close/reopen before continuing.`;input.disabled=send.disabled=true;}
  finally{if(polling===ticket)polling=null;}
}
open.addEventListener('click',async()=>{
  generation++;pasteReview.reset();history.reset();open.disabled=true;button.disabled=true;
  try{epoch=await invoke('open_shell');keyQueue={chain:Promise.resolve(),pending:0,locked:false};state=null;resize.disabled=close.disabled=input.disabled=send.disabled=false;status.textContent='Experimental /bin/sh; TERM=dumb. Focus terminal for experimental keyboard input; no native GUI/IME acceptance.';timer=setInterval(poll,50);await poll();}
  catch(e){status.textContent=String(e?.message??e);open.disabled=false;}
});
close.addEventListener('click',async()=>{
  generation++;pasteReview.reset();ime.reset();history.reset();clearInterval(timer);const old=epoch;epoch=null;resize.disabled=close.disabled=input.disabled=send.disabled=true;
  try{const result=await invoke('close_shell',{epoch:old});status.textContent=result?.cleanup_pending===false?'Terminal closed; direct child reaped. Descendant containment is not provided.':'Terminal closed; direct-child cleanup handed to bounded supervisor and may still be pending. Descendant containment is not provided.';state=null;open.disabled=false;button.disabled=false;}
  catch(e){epoch=old;close.disabled=false;status.textContent=`Close error: ${String(e?.message??e)}. Retry Close to finish cleanup.`;}
});
document.querySelector('#command').addEventListener('submit',async event=>{
  event.preventDefault();if(epoch===null)return;
  // Reserve one byte for the appended newline before creating an encoded copy.
  if(input.value.length>4095){status.textContent='Input exceeds 4096-byte bound';return;}
  const bytes=new TextEncoder().encode(input.value+'\n');if(bytes.length>4096){status.textContent='Input exceeds 4096-byte bound';return;}
  send.disabled=true;input.disabled=true;const current=epoch;let at=0;let failed=false;
  try{await keyQueue.chain;if(epoch!==current||keyQueue.locked)throw new Error('input unavailable');for(let attempts=0;at<bytes.length&&attempts<128;attempts++){
    if(epoch!==current)throw new Error('session changed');
    if(keyQueue.locked)throw new Error('input became unavailable');
    const n=await invoke('write_shell',{epoch:current,bytes:Array.from(bytes.subarray(at))});
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
const sizes=resizeQueue({getEpoch:()=>epoch,invoke,onDone:async()=>{await poll();},onError:e=>{status.textContent=String(e?.message??e);}});
resize.addEventListener('click',async()=>{if(epoch===null||input.disabled)return;try{await sizes.request(Number(rowsInput.value),Number(colsInput.value));}catch(e){status.textContent=String(e?.message??e);}});
function describeCursor(){if(state)document.querySelector('#cursor-status').textContent=`${state.cols} × ${state.rows.length} cells · cursor row ${state.cursor_row+1}, column ${state.cursor_col+1} · ${state.cursor_shape}${state.cursor_visible?'':' (hidden)'}${state.pending_wrap?' · wrap pending':''}`;}
document.querySelector('#reveal-cursor').addEventListener('click',()=>{if(!state)return;const viewport=document.querySelector('.viewport');viewport.scrollTo(cursorTarget(state,canvas,viewport));});


// Browser supplies logical events only; Rust chooses all terminal wire sequences.
let keyQueue={chain:Promise.resolve(),pending:0,locked:false};
function enqueueLogical(command,payload){
  if(epoch===null||input.disabled)return;
  const queue=keyQueue;
  if(queue.pending>=128){pasteReview.reset();queue.locked=true;status.textContent='Keyboard queue full; input locked. Close/reopen before retrying.';input.disabled=send.disabled=true;return;}
  const current=epoch;queue.pending++;
  queue.chain=queue.chain.then(async()=>{
    if(epoch!==current||queue.locked)return;
    try{await invoke(command,{epoch:current,...payload});}
    catch(e){if(epoch===current){pasteReview.reset();queue.locked=true;input.disabled=send.disabled=true;status.textContent=`Input not accepted: ${String(e?.message??e)}. Close/reopen before retrying.`;}}
  }).finally(()=>{queue.pending--;});
}
function keydown(event){
  if(epoch===null||input.disabled||ime.isComposing()||event.isComposing||event.keyCode===229||event.getModifierState?.('AltGraph')||event.metaKey||event.key==='Dead'||event.key==='Process')return;
  // Leave browser paste shortcut alone; the paste event carries clipboard text.
  if((event.ctrlKey||event.metaKey)&&event.key.toLowerCase()==='v')return;
  const named=/^(Arrow(Up|Down|Left|Right)|Home|End|Insert|Delete|PageUp|PageDown|F([1-9]|1[0-2])|Enter|Escape|Backspace|Tab)$/;
  if(!named.test(event.key)&&[...event.key].length!==1)return;
  if(event.currentTarget===imeInput&&!named.test(event.key)&&!event.ctrlKey&&!event.altKey)return;
  event.preventDefault();enqueueLogical('key_shell',{name:event.key,shift:event.shiftKey,alt:event.altKey,ctrl:event.ctrlKey});
}
canvas.addEventListener('keydown',keydown);imeInput.addEventListener('keydown',keydown);
function paste(event){
  if(epoch===null||input.disabled)return;
  event.preventDefault();const text=event.clipboardData?.getData('text/plain')||'';
  pasteReview.offer(text);
}
canvas.addEventListener('paste',paste);imeInput.addEventListener('paste',paste);

document.querySelector('#copy-visible').addEventListener('click',()=>{
  if(!state)return;
  history.reset();
  const output=document.querySelector('#history-output');
  output.value=state.rows.map(row=>row.filter(cell=>cell.width!==0).map(cell=>cell.text).join('').trimEnd()).join('\n');
  document.querySelector('#history-label').textContent=`Visible grid snapshot ${state.revision}. Select and copy normally; visual rows are separated by newlines.`;
});

document.querySelector('#recover').addEventListener('click',async()=>{
  if(epoch===null||recovery?.epoch===epoch)return;
  const current=epoch, ticket=++transport, operation={epoch:current};recovery=operation;clearInterval(timer);polling=null;
  try {
    const frame=await invoke('recover_shell',{epoch:current});
    if(epoch!==current||transport!==ticket)return;
    const next=acceptFull(null,frame,{session:'pty-demo',epoch:current});
    paint(canvas,next);state=next;describeCursor();
    await invoke('ack_shell',{epoch:current,revision:next.revision});
    if(epoch!==current||transport!==ticket)return;
    status.textContent='Output snapshot recovered. Existing input safety locks remain unchanged.';
    timer=setInterval(poll,50);await poll();
  }catch(e){if(epoch===current&&transport===ticket)status.textContent=`Recovery failed: ${String(e?.message??e)}. Retry recovery or close.`;}
  finally{if(recovery===operation)recovery=null;}
});
