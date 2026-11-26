import {installTerminalSelection} from './terminal-selection.js';
import {validateReloadView,retainedReloadView,saveReloadTicket,takeReloadTicket} from './ssh-reload.js';
import {installFilePreviewPanel} from './file-preview-panel.js';
import {openVim9Snapshot} from './vim9-workbench.js';
import {installPasteReview} from './paste.js';
import {dimensions,resizeQueue,cursorTarget,terminalCellAt,viewportDimensions} from './viewport.js';
import {installHistory} from './history.js';
import {installCommandBlocks} from './command-blocks.js';
import {installTerminalChat,validateChatStatus} from './terminal-chat.js';
import {installWorkflows} from './workflows.js';
import {plainLocalTab,readLocalShellLayout,saveLocalShellLayout,validPaneId,MAX_LOCAL_SHELLS,validLocalGeometry,localSplitPair} from './local-layout.js';
import {acceptFull,acceptUpdate,clearPaint,paint} from './frames.js'; import {installIme} from './ime.js';
import {acceptChannelFrame,channelCounter,CHANNEL_FRAME_VERSION} from './frames-v4.js';
const canvas=document.querySelector('canvas'),status=document.querySelector('#status'),button=document.querySelector('#demo');
const open=document.querySelector('#open'),attach=document.querySelector('#attach'),detach=document.querySelector('#detach'),close=document.querySelector('#close'),input=document.querySelector('#input'),send=document.querySelector('#send');
const openIntegrated=document.querySelector('#open-integrated');
const sshForm=document.querySelector('#ssh-terminal-form'),sshHost=document.querySelector('#ssh-terminal-host'),openSsh=document.querySelector('#open-ssh');
const localTools=document.querySelector('#local-shells'),showLocal=document.querySelector('#manage-local'),localOptions=document.querySelector('#local-shell-options'),localSelection=document.querySelector('#local-shell-selection'),attachLocal=document.querySelector('#attach-selected-local'),openSecond=document.querySelector('#open-second-local');
const tabs=document.querySelector('#local-tabs'),tabList=document.querySelector('#local-tab-list'),newTab=document.querySelector('#new-local-tab'),retryTab=document.querySelector('#retry-local-tab');
const newJshTab=document.querySelector('#new-jsh-tab');
const splitToggle=document.querySelector('#split-local'),panes=document.querySelector('#terminal-panes'),peerPane=document.querySelector('#peer-pane'),peerCanvas=document.querySelector('#peer-canvas'),focusPeer=document.querySelector('#focus-peer-pane');
const savedLayoutPanel=document.querySelector('#saved-local-layout'),restoreLayout=document.querySelector('#restore-local-layout');
let roster=[],localListRequest=0;
const localDrafts=new Map();
const localTabEpochs=new Set();
const localTabOwners=new Map();
const localShellKinds=new Map();
const localPaneIds=new Map();
const localTabGeometry=new Map();
let changingTab=null,restoringLocalLayout=false;
let state=null,epoch=null,owner=null,timer=null,polling=null,generation=0,transport=0,recovery=null;
let sshSession=null,sshInputSuspended=false,detachedLocal=null,sshReloadNeeded=false,retainedLayoutBlocked=false;
const showSavedSsh=document.querySelector('#show-saved-ssh'),savedSshPanel=document.querySelector('#saved-ssh');
const savedSshSelect=document.querySelector('#saved-ssh-shell'),resumeSavedSsh=document.querySelector('#resume-saved-ssh'),forgetSavedSsh=document.querySelector('#forget-saved-ssh');
let savedSshShells=[],savedSshRequest=0;
let localStream=null,localTransport='channel',localInputSuspended=false,localFrameRecovery=false;
let splitEnabled=false,splitEpochs=[],splitLayout=1,settingSplit=false,peerState=null;
const selectionOverlay = document.querySelector('#terminal-selection');
const copySelection = document.querySelector('#copy-selection');
const liveSelection = selectionOverlay && typeof canvas.setPointerCapture === 'function'
  ? installTerminalSelection({
    canvas, overlay: selectionOverlay, document,
    onFrame(frame) {
      if (frame) paint(canvas, frame);
      else clearPaint(canvas);
      describeCursor(frame);
    },
    onSelection(available) { if (copySelection) copySelection.disabled = !available; },
  })
  : null;
function paintLiveFrame(frame) {
  if (liveSelection) liveSelection.present(frame);
  else paint(canvas, frame);
}
const visibleLiveFrame = () => liveSelection?.visibleFrame() ?? state;
const filePreview=installFilePreviewPanel(document,{getFrame:visibleLiveFrame,getEpoch:()=>changingTab?null:epoch,getSession:()=>changingTab?null:owner?.session,
  getOwner:()=>!changingTab&&!recovery&&owner?JSON.stringify([owner.session,owner.epoch,owner.generation]):null,isLocalTerminal:()=>!sshSession,
  openInVim:typeof openVim9Snapshot==='function'?openVim9Snapshot:null});
const imeInput=document.querySelector('#ime-input');
const terminalPreedit=document.querySelector('#terminal-preedit');
const terminalChatSend=document.querySelector('#terminal-chat-send');
const inputOwner=()=>!input.disabled?owner:null;
function refreshTerminalControls(){terminalChat.updateOwner();workflows.updateOwner();}
const ime=installIme(imeInput,{getEpoch:inputOwner,
canCommit:()=>!document.hidden&&(document.activeElement??imeInput)===imeInput,
afterEnd:callback=>{
  const flush=()=>{callback();refreshTerminalControls();};
  if(typeof requestAnimationFrame==='function')requestAnimationFrame(flush);else flush();
},preedit:text=>{
  terminalPreedit.textContent=text;terminalPreedit.hidden=text==='';
  void Promise.resolve().then(refreshTerminalControls);
},commit:(text,attachment)=>{if(attachment!==owner)return;if(text.length>4096||new TextEncoder().encode(text).length>4096){status.textContent='Committed text exceeds 4096-byte limit; nothing sent.';return;}enqueueLogical('text_shell',{text});}});
// WebKit can emit compositionend with the uncommitted preedit while the
// browser moves focus, before the text element receives its blur event.
document.addEventListener?.('pointerdown',event=>{
  if(event.target===terminalChatSend){event.preventDefault();return;}
  if(event.target!==imeInput&&event.target!==canvas&&ime.isComposing())ime.reset();
},true);
terminalChatSend.addEventListener('mousedown',event=>event.preventDefault());
document.addEventListener?.('keydown',event=>{
  if(event.key==='Tab'&&!event.isComposing&&event.keyCode!==229&&ime.isComposing())ime.reset();
},true);
globalThis.addEventListener?.('blur',()=>ime.reset());
canvas.addEventListener('click',event=>{if(!event.defaultPrevented&&!event.ctrlKey)imeInput.focus();});
const resize=document.querySelector('#resize');
const invoke=globalThis.__TAURI__?.core?.invoke;
const Channel=globalThis.__TAURI__?.core?.Channel;
const channelSnapshots=()=>typeof Channel==='function'?{frameVersion:CHANNEL_FRAME_VERSION}:{};
const acceptAttachedSnapshot=(frame,attachment,options)=>options.frameVersion===CHANNEL_FRAME_VERSION
  ?acceptChannelFrame(null,frame,{...attachment,frameVersion:options.frameVersion},true):acceptFull(null,frame,attachment);
let savedLayout=invoke?readLocalShellLayout():null;
const sshCommands={poll_shell:'poll',ack_shell:'ack',shell_status:'status',history_shell:'history',recover_shell:'recover',detach_shell:'detach',close_shell:'close',write_shell:'write',key_shell:'key',text_shell:'text',paste_shell:'paste',resize_shell:'resize'};
function setOpenDisabled(disabled) {
  open.disabled=openIntegrated.disabled=openSsh.disabled=sshHost.disabled=disabled||!invoke;
  showSavedSsh.disabled=openSsh.disabled;
  if(showSavedSsh.disabled){savedSshRequest++;savedSshPanel.hidden=true;}
  updateSavedSshSelection();
  setLocalTools(!disabled);
  renderSavedLayout();
}
function setLocalTools(available) {
  showLocal.disabled=!available||!invoke||sshSession!==null||restoringLocalLayout;
  if(showLocal.disabled){
    localListRequest++;roster=[];localOptions.hidden=true;
    attachLocal.disabled=openSecond.disabled=true;
  }
}
function renderSavedLayout(){
  savedLayoutPanel.hidden=retainedLayoutBlocked||!savedLayout||owner!==null||sshSession!==null||localTabEpochs.size>0;
  restoreLayout.disabled=retainedLayoutBlocked||open.disabled||restoringLocalLayout;
}
function rememberLocalGeometry(frame){
  if(!frame||!localTabEpochs.has(frame.epoch)||localTabOwners.get(frame.epoch)?.session!==frame.session)return;
  const geometry={cols:frame.cols,rows:frame.rows.length};
  if(!validLocalGeometry(geometry))return;
  const old=localTabGeometry.get(frame.epoch);
  if(old?.rows===geometry.rows&&old?.cols===geometry.cols)return;
  localTabGeometry.set(frame.epoch,geometry);recordLocalLayout();
}
function updateSplitPair(previous=null){
  if(splitEnabled)splitEpochs=localSplitPair([...localTabEpochs].sort((a,b)=>a-b),epoch,splitEpochs,previous);
}
function recordLocalLayout(){
  if(restoringLocalLayout||changingTab||settingSplit)return;
  const epochs=[...localTabEpochs].sort((a,b)=>a-b);
  const configurations=[];
  for(const number of epochs){
    const session=localTabOwners.get(number)?.session;
    const shell=localShellKinds.get(session);
    if(!shell)return;
    configurations.push({shell,presentation:terminalChat.modeFor(session),paneId:localPaneIds.get(session)});
  }
  const active=Math.max(0,epochs.indexOf(epoch));
  const split=splitEnabled&&splitEpochs.length===2&&splitEpochs.every(number=>epochs.includes(number));
  const complete=epochs.every(number=>localTabGeometry.has(number));
  if(epochs.length>2&&!complete)return;
  savedLayout=epochs.length?{tabs:configurations,active,split}:null;
  if(savedLayout&&complete){
    savedLayout.tabs=configurations.map((tab,index)=>({...tab,geometry:localTabGeometry.get(epochs[index])}));
    savedLayout.panes=split?splitEpochs.map(number=>epochs.indexOf(number)):[];
  }
  saveLocalShellLayout(savedLayout);renderSavedLayout();
}
function observerTarget(){
  if(!splitEnabled||!owner||sshSession!==null||localTabEpochs.size<2||!splitEpochs.includes(epoch))return null;
  const other=splitEpochs.find(number=>number!==epoch&&localTabEpochs.has(number));
  return other===undefined?null:{epoch:other,layout:splitLayout};
}
function renderSplit(){
  const target=observerTarget();
  splitToggle.disabled=!owner||sshSession!==null||localTabEpochs.size<2||!!changingTab||!!recovery||restoringLocalLayout||settingSplit;
  splitToggle.setAttribute('aria-pressed',String(!!target));
  splitToggle.textContent=target?'Single pane':'Split panes';
  panes.dataset.split=target?(splitEpochs[0]===epoch?'left':'right'):'';
  peerPane.hidden=!target;
  focusPeer.disabled=splitToggle.disabled;
  if(target)focusPeer.textContent=`Focus Shell ${target.epoch}`;
  if(peerState&&peerState.epoch!==target?.epoch){
    peerState=null;clearPaint(peerCanvas);delete peerCanvas.dataset.geometry;
  }
}
function renderLocalTabs(focusActive=false) {
  renderSavedLayout();
  renderSplit();
  history.updateSearchControls();
  blocks.updateOwner();
  refreshTerminalControls();
  tabs.hidden=!invoke||sshSession!==null||!localTabEpochs.size||(!owner&&showLocal.disabled&&!changingTab);
  if(tabs.hidden)return;
  let activeButton=null;
  const buttons=[...localTabEpochs].sort((a,b)=>a-b).map(number=>{
    const tab=document.createElement('button');
    const active=number===epoch;
    tab.type='button';tab.dataset.epoch=String(number);tab.textContent=`Shell ${number}`;
    if(active){tab.setAttribute('aria-current','true');activeButton=tab;}
    tab.disabled=changingTab!==null||recovery!==null||restoringLocalLayout;
    tab.addEventListener('click',()=>{if(!tab.disabled&&!active)startLocalTab(number);});
    return tab;
  });
  tabList.replaceChildren(...buttons);
  newTab.disabled=newJshTab.disabled=!owner||changingTab!==null||recovery!==null||restoringLocalLayout||localTabEpochs.size>=MAX_LOCAL_SHELLS;
  retryTab.hidden=!changingTab||changingTab.busy||restoringLocalLayout;
  if(focusActive)activeButton?.focus();
}
function validatePeerFrame(frame,target){
  const session=localTabOwners.get(target.epoch)?.session??frame?.session;
  if(typeof session!=='string'||!/^[a-f0-9]{32}$/.test(session))throw new Error('invalid inactive shell');
  const next=acceptFull(null,frame,{session,epoch:target.epoch});
  return peerState?.epoch===next.epoch&&peerState.revision>=next.revision?null:next;
}
function validateChannelPeer(frame,target){
  const session=localTabOwners.get(target.epoch)?.session;
  if(typeof session!=='string'||!/^[a-f0-9]{32}$/.test(session))throw new Error('invalid inactive shell');
  const next=acceptChannelFrame(null,frame,{session,epoch:target.epoch,frameVersion:CHANNEL_FRAME_VERSION},true);
  return peerState?.epoch===next.epoch&&BigInt(peerState.revision)>=BigInt(next.revision)?null:next;
}
function paintPeer(frame){
  if(!frame)return;
  paint(peerCanvas,frame);peerState=frame;
  peerCanvas.dataset.geometry=`${frame.cols}x${frame.rows.length}`;
  rememberLocalGeometry(frame);
}
function sameObservation(left,right){
  return left?.epoch===right?.epoch&&left?.layout===right?.layout;
}
async function syncObserver(stream){
  if(localStream!==stream||!stream.ready)return;
  if(stream.adjusting)return stream.adjusting;
  stream.adjusting=(async()=>{
    let target=observerTarget();
    while(localStream===stream&&!sameObservation(stream.observer,target)){
      await invoke('set_local_observer',{owner:stream.owner,streamId:stream.channel.id,observer:target});
      stream.observer=target;target=observerTarget();
    }
  })();
  try{await stream.adjusting;}finally{stream.adjusting=null;}
}
splitToggle.addEventListener('click',async()=>{
  if(splitToggle.disabled)return;
  splitEnabled=!splitEnabled;splitLayout++;
  splitEpochs=splitEnabled?[...localTabEpochs].filter(number=>number!==epoch).slice(0,1).concat(epoch).sort((a,b)=>a-b):[];
  settingSplit=true;renderSplit();recordLocalLayout();
  const attachment=owner;
  try{
    if(localStream)await syncObserver(localStream);
    else if(observerTarget()&&localTransport!=='recovery'){
      clearInterval(timer);timer=setInterval(poll,50);await poll();
    }
  }
  catch(error){if(owner===attachment)status.textContent=`Split pane unavailable: ${String(error?.message??error)}`;}
  finally{settingSplit=false;renderSplit();recordLocalLayout();}
});
function selectPeer(){
  const target=observerTarget();
  if(target&&!focusPeer.disabled)return startLocalTab(target.epoch,{focusTerminal:true});
}
focusPeer.addEventListener('click',selectPeer);
peerCanvas.addEventListener('click',selectPeer);
function setTabRoster(records){
  for(const [number,attachment] of localTabOwners){
    if(records.includes(number))continue;
    localTabOwners.delete(number);localDrafts.delete(attachment.session);
    localShellKinds.delete(attachment.session);localPaneIds.delete(attachment.session);localTabGeometry.delete(number);terminalChat.forget(attachment.session);
  }
  localTabEpochs.clear();
  for(const number of records)localTabEpochs.add(number);
  updateSplitPair();renderLocalTabs();
}
function rememberLocalDraft(attachment) {
  if(input.value)localDrafts.set(attachment.session,input.value);
  else localDrafts.delete(attachment.session);
}
function restoreLocalDraft(attachment) {
  input.value=localDrafts.get(attachment.session)??'';
}
function rememberLocalTab(attachment,configuration,paneId){
  localTabEpochs.add(attachment.epoch);
  localTabOwners.set(attachment.epoch,attachment);
  if(paneId!==undefined)localPaneIds.set(attachment.session,paneId);
  if(configuration){
    localShellKinds.set(attachment.session,configuration.shell);
    terminalChat.restoreMode(attachment.session,configuration.presentation);
  }
}
function selectLocal(){
  return roster.find(epoch=>String(epoch)===localSelection.value)??null;
}
function updateLocalSelection(){
  const target=selectLocal();
  attachLocal.disabled=target===null||target===epoch;
}
async function listLocalShellEpochs(){
  const records=await invoke('list_local_shells');
  if(!Array.isArray(records)||records.length>MAX_LOCAL_SHELLS)throw new Error('invalid local shell list');
  if(records.some(epoch=>!Number.isInteger(epoch)||epoch<=0||epoch>0xffffffff)||
      new Set(records).size!==records.length)throw new Error('invalid retained local shell');
  return records;
}
showLocal.addEventListener('click',async()=>{
  if(showLocal.disabled)return;
  const request=++localListRequest,attempt=generation;
  localOptions.hidden=true;attachLocal.disabled=openSecond.disabled=true;roster=[];
  try{
    const records=await listLocalShellEpochs();
    if(request!==localListRequest||generation!==attempt||showLocal.disabled)return;
    roster=records;
    if(!records.length)retainedLayoutBlocked=false;
    setTabRoster(records);
    const options=roster.map(epoch=>{
      const option=document.createElement('option');option.value=String(epoch);
      option.textContent='Local shell '+epoch+(epoch===owner?.epoch?' (current)':'');return option;
    });
    localSelection.replaceChildren(...options);
    localSelection.value=epoch===null&&roster.length===1?String(roster[0]):'';
    updateLocalSelection();
    attachLocal.textContent=epoch===null?'Reconnect selected view-only':'Switch to selected view-only';
    openSecond.disabled=epoch!==null||roster.length!==1;localOptions.hidden=false;
    if(epoch===null){
      if(!roster.length)status.textContent='No retained local shell.';
      else if(roster.length>1)status.textContent='Choose which local shell to reconnect view-only.';
    }
  }catch(e){
    if(request!==localListRequest||generation!==attempt||showLocal.disabled)return;
    roster=[];status.textContent=String(e?.message??e);
  }
});
localSelection.addEventListener('change',updateLocalSelection);

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
function acceptLocalPane(result){
  const pane=result?.pane_history_id;
  if(pane!==undefined&&!validPaneId(pane))throw new Error('Invalid completed command pane identity.');
  return pane;
}
const pasteReviewPanel=document.querySelector('#paste-review');
const pasteReview=installPasteReview({panel:pasteReviewPanel,preview:document.querySelector('#paste-preview'),escaped:document.querySelector('#paste-escaped'),confirm:document.querySelector('#paste-confirm'),cancel:document.querySelector('#paste-cancel'),getEpoch:inputOwner,send:(text,attachment)=>{if(attachment===owner)enqueueLogical('paste_shell',{text});},onError:message=>{status.textContent=message;}});
const history=installHistory({older:document.querySelector('#history-older'),newer:document.querySelector('#history-newer'),refresh:document.querySelector('#history-refresh'),output:document.querySelector('#history-output'),label:document.querySelector('#history-label'),
 invoke:(command,{owner:attachment,...args})=>invokeAttached(command,{epoch:attachment.epoch,...args},attachment),
 getOwner:()=>changingTab?null:owner,
 visual:{viewport:document.querySelector('#history-viewport'),spacer:document.querySelector('#history-grid-size'),canvas:document.querySelector('#history-canvas')},
 copyControls:{button:document.querySelector('#history-copy'),status:document.querySelector('#history-copy-status'),
  write:text=>invoke('copy_terminal_text',{text})},
 enableProjection:attachment=>typeof Channel==='function'&&attachment.session!==sshSession,
 searchControls:{form:document.querySelector('#history-find-form'),query:document.querySelector('#history-find-query'),
  find:document.querySelector('#history-find'),older:document.querySelector('#history-find-older'),
  cancel:document.querySelector('#history-find-cancel'),status:document.querySelector('#history-find-status'),
  enabled:attachment=>attachment.session!==sshSession&&!restoringLocalLayout&&!recovery}});
const localView=()=>owner&&owner.session!==sshSession&&!changingTab&&!recovery&&!restoringLocalLayout?owner:null;
const blocks=installCommandBlocks(document,{
  copyText:text=>invoke('copy_terminal_text',{text}),
  getOwner:localView,
  read:attachment=>invokeAttached('local_command_blocks',{epoch:attachment.epoch},attachment),
  bookmark:(attachment,selection,bookmarked)=>invokeAttached('set_local_command_bookmark',{
    epoch:attachment.epoch,selection,bookmarked,
  },attachment),
  find:(attachment,query)=>invokeAttached('search_local_blocks',{epoch:attachment.epoch,query},attachment),
  compare:(attachment,before,after)=>invokeAttached('compare_local_output',{epoch:attachment.epoch,before,after},attachment),
  exportReview:(reviewId,format)=>invoke('export_local_output',{reviewId,format}),
  recall:(block,attachment,origin,report)=>{
    if(inputOwner()!==attachment){report('The selected terminal is read-only.');return;}
    pasteReview.offer(block.command,origin,(_text,expected)=>{
      if(inputOwner()!==attachment||expected!==attachment)return;
      enqueueLogical('recall_local_command',{selection:{sequence:block.sequence,revision:block.revision}},reason=>{
        report(reason??'Command inserted at the shell prompt.');
        if(!reason)imeInput.focus();
      });
    });
  },
  returnToTerminal:()=>{(input.disabled?canvas:imeInput).focus();},
});
const terminalChat=installTerminalChat(document,{
  getOwner:localView, blocks,
  onModeChange:recordLocalLayout,
  canSend:()=>!!localView()&&!input.disabled&&ime.isSettled(),
  sendEnter:(prompt,receipt)=>{
    if(!pasteReviewPanel.hidden)return;
    return enqueueLogical('submit_local_chat',{prompt},accepted=>{
      receipt(accepted===true);
      if(accepted!==true)status.textContent='The shell prompt changed. Continue typing in the live terminal.';
    });
  },
  focusTerminal:()=>imeInput.focus(),
  revealCursor:revealLiveCursor,
});
const workflows=installWorkflows(document,{
  context:()=>{
    const attachment=localView(),prompt=terminalChat.currentPrompt();
    return attachment&&inputOwner()===attachment&&prompt&&ime.isSettled()&&pasteReviewPanel.hidden
      ?{owner:attachment,prompt}:null;
  },
  retained:attachment=>localTabOwners.get(attachment.epoch)?.session===attachment.session,
  invoke:request=>invoke('local_workflow_library',{request}),
  insert:async(choice,target,receipt)=>{
    // Keep keyboard focus out of the PTY until insertion has settled. A
    // repeated Enter from the form must not execute the inserted line.
    if(owner?.session!==target.session)await startLocalTab(target.epoch);
    const attachment=inputOwner();
    if(changingTab||attachment?.session!==target.session||attachment.epoch!==target.epoch||!ime.isSettled()||!pasteReviewPanel.hidden){
      throw new Error('The original shell is unavailable for input.');
    }
    return enqueueLogical('insert_local_workflow',choice,receipt);
  },
  returnToTerminal:()=>{(input.disabled?canvas:imeInput).focus();},
  announce:message=>{status.textContent=message;},
});
openIntegrated.hidden=!invoke;
sshForm.hidden=!invoke;localTools.hidden=!invoke;
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
    const next=acceptFull(state,frame);paintLiveFrame(next);state=next;describeCursor();
    if(invoke) await invoke('acknowledge',{session:state.session,epoch:state.epoch,revision:state.revision});
    status.textContent=`Rust frame ${state.revision} accepted. No PTY attached.`;
  }catch(e){if(currentGeneration!==generation)return;status.textContent=`Stopped: ${String(e?.message??e)}. Restart the app to reset.`;}
  finally {if(currentGeneration===generation){button.disabled=!invoke&&state!==null;setOpenDisabled(false);attach.disabled=!invoke;}}
});
function validateShellStatus(info,expectedEpoch){
  if(!info||info.epoch!==expectedEpoch||typeof info.exited!=='boolean'||typeof info.drained!=='boolean'||(info.write_error!==null&&typeof info.write_error!=='string')||typeof info.dropped_reply_bytes!=='string'||!/^[0-9]{1,20}$/.test(info.dropped_reply_bytes)||typeof info.unsupported!=='string'||!/^[0-9]{1,20}$/.test(info.unsupported))throw new Error('invalid session status');
  const exitNotice=info.exit_output_notice;
  if(exitNotice!=null&&(typeof exitNotice!=='string'||exitNotice.length>256))throw new Error('invalid exit output notice');
  if(owner?.session!==sshSession){
    validateChatStatus(info.chat);
    acceptLocalPane(info);
    if(info.history_oldest_row!==undefined)channelCounter(info.history_oldest_row);
  }
}
function showShellStatus(info,mode){
  const health=document.querySelector('#health');
  health.dataset.transport=mode;
  if(owner&&owner.session!==sshSession&&typeof info.command_blocks_enabled==='boolean'){
    const shell=info.command_blocks_enabled?'jsh':'plain';
    const pane=info.pane_history_id;
    if(localShellKinds.get(owner.session)!==shell||localPaneIds.get(owner.session)!==pane){
      localShellKinds.set(owner.session,shell);
      if(pane===undefined)localPaneIds.delete(owner.session);
      else localPaneIds.set(owner.session,pane);
      recordLocalLayout();
    }
  }
  health.textContent=`Unsupported sequences: ${info.unsupported}. Dropped reply bytes: ${info.dropped_reply_bytes}. Shell ${info.exited?'exited; output remains visible':'running'}.${info.write_error?' Input disabled: '+info.write_error:''}`;
  if(info.exit_output_notice)health.textContent+=' '+info.exit_output_notice;
  if(info.exited||info.write_error){pasteReview.reset();keyQueue.locked=true;input.disabled=send.disabled=resize.disabled=true;}
  if(owner&&owner.session!==sshSession){
    terminalChat.showStatus(info,owner);
    if(info.history_oldest_row!==undefined)history.updateRetained(owner,info.history_oldest_row);
  }
  workflows.updateOwner();
  if(document.activeElement===imeInput||document.activeElement===canvas)terminalChat.followCursor();
}
function stopTransport(){
  clearInterval(timer);
  if(localStream){
    if(localStream.animation!==null)cancelAnimationFrame(localStream.animation);
    localStream.channel.onmessage=()=>{};
  }
  localStream=null;
}
function failLocalStream(stream,error,recoverFrame=false){
  if(localStream!==stream||owner!==stream.owner)return;
  suspendLocalOutput(error,recoverFrame);
}
function suspendLocalOutput(error,recoverFrame=false){
  stopTransport();localTransport='recovery';localFrameRecovery=recoverFrame;
  localInputSuspended=!keyQueue.locked&&!keyQueue.pending&&!input.disabled;
  pasteReview.reset();ime.reset();keyQueue.locked=true;
  input.disabled=send.disabled=resize.disabled=true;
  refreshTerminalControls();
  status.textContent=`Local output stream paused: ${String(error?.message??error)}. Recover the output snapshot to continue.`;
}
async function receiveLocalStream(stream,event){
  if(localStream!==stream||owner!==stream.owner)return;
  let sequence=null,recoverFrame=false;
  try{
    const attachment=acceptOwner(event?.owner);
    if(attachment.session!==owner.session||attachment.epoch!==epoch||attachment.generation!==owner.generation)
      throw new Error('foreign local stream');
    if(event.kind==='stopped'&&typeof event.message==='string')throw new Error(event.message);
    if(event.kind!=='update')throw new Error('invalid local stream event');
    sequence=channelCounter(event.sequence);
    if(sequence!==stream.sequence+1n)throw new Error('invalid local stream sequence');
    validateShellStatus(event.status,attachment.epoch);
    const target=observerTarget();
    recoverFrame=true;
    const peer=target&&event.preview?.layout===target.layout?validateChannelPeer(event.preview.frame,target):null;
    let revision=null;
    if(event.frame!==null){
      const next=acceptChannelFrame(stream.frame,event.frame,{...attachment,frameVersion:CHANNEL_FRAME_VERSION});
      paintLiveFrame(next);state=next;stream.frame=next;revision=next.revision;describeCursor();
    }
    paintPeer(peer);
    recoverFrame=false;
    showShellStatus(event.status,'channel');
    stream.sequence=sequence;stream.lastSeen=Date.now();
    const acknowledging=invoke('ack_local_stream',{owner:stream.owner,streamId:stream.channel.id,sequence:event.sequence,revision});
    stream.acknowledging=acknowledging;
    await acknowledging;
    if(stream.acknowledging===acknowledging)stream.acknowledging=null;
    if(localStream===stream&&event.status.drained&&stream.sequence===sequence)clearInterval(timer);
  }catch(error){
    // A later delivery proves that the previous acknowledgement was applied,
    // even when its IPC response arrived late or was lost.
    if(sequence===null||sequence>=stream.sequence)failLocalStream(stream,error,recoverFrame);
  }
}
function queueLocalStream(stream,event){
  if(localStream!==stream||owner!==stream.owner)return;
  if(event?.kind!=='update'||(!event.frame&&!event.preview)||typeof requestAnimationFrame!=='function')
    return receiveLocalStream(stream,event);
  if(stream.animation!==null){failLocalStream(stream,new Error('local stream delivered another frame before acknowledgement'));return;}
  stream.animation=requestAnimationFrame(()=>{
    stream.animation=null;
    void receiveLocalStream(stream,event);
  });
}
async function startTransport(){
  if(!owner||localTransport==='recovery')return;
  let channel;
  if(owner.session!==sshSession&&localTransport!=='poll'&&typeof Channel==='function'){
    try{channel=new Channel();}catch{/* No subscription was sent. */}
  }
  if(!channel){
    if(owner.session!==sshSession&&state?.version===CHANNEL_FRAME_VERSION){
      suspendLocalOutput(new Error('local output Channel unavailable'));
      return;
    }
    timer=setInterval(poll,50);await poll();return;
  }
  const stream={owner,channel,sequence:0n,frame:null,lastSeen:Date.now(),animation:null,observer:observerTarget(),ready:false,adjusting:null};
  localStream=stream;
  stream.channel.onmessage=event=>queueLocalStream(stream,event);
  timer=setInterval(()=>{
    if(localStream===stream&&!document.hidden&&Date.now()-stream.lastSeen>10000)
      failLocalStream(stream,new Error('no local output heartbeat'));
  },1000);
  try{await invoke('subscribe_local_shell',{owner:stream.owner,stream:stream.channel,observer:stream.observer,frameVersion:CHANNEL_FRAME_VERSION});}
  catch(error){failLocalStream(stream,error);}
  if(localStream===stream){
    stream.ready=true;
    try{await syncObserver(stream);}
    catch(error){if(localStream===stream)status.textContent=`Split pane unavailable: ${String(error?.message??error)}`;}
  }
}
document.addEventListener?.('visibilitychange',()=>{if(document.hidden)ime.reset();if(localStream)localStream.lastSeen=Date.now();});

async function poll(){
  if(epoch===null||changingTab||localStream||localTransport==='recovery'||polling?.epoch===epoch)return;const current=epoch,attachment=owner,ticket={epoch,transport};polling=ticket;
  try{const frame=await invokeAttached('poll_shell',{epoch:current},attachment);if(epoch!==current||ticket.transport!==transport)return;
    if(frame){const next=acceptUpdate(state,frame,attachment);paintLiveFrame(next);state=next;describeCursor();await invokeAttached('ack_shell',{epoch:current,revision:next.revision},attachment);}
    if(epoch!==current||ticket.transport!==transport)return;
    const info=await invokeAttached('shell_status',{epoch:current},attachment);if(epoch!==current||ticket.transport!==transport)return;
    validateShellStatus(info,current);showShellStatus(info,'poll');
    const target=observerTarget();
    if(target){
      try{
        const preview=await invoke('observe_local_shell',{owner:attachment,epoch:target.epoch,revision:peerState?.revision??null});
        if(owner===attachment&&sameObservation(target,observerTarget())&&preview)paintPeer(validatePeerFrame(preview,target));
      }catch(error){if(owner===attachment&&sameObservation(target,observerTarget()))status.textContent=`Split pane unavailable: ${String(error?.message??error)}`;}
    }
    // Continue bounded polling after child exit so trailing PTY bytes are drained.
    if(info.drained&&!target)clearInterval(timer);

  }catch(e){
    if(epoch!==current||ticket.transport!==transport)return;
    const isSsh=attachment.session===sshSession;
    sshInputSuspended=isSsh&&!keyQueue.locked&&!keyQueue.pending&&!input.disabled;
    sshReloadNeeded=isSsh&&sshInputSuspended&&ime.isComposing();
    pasteReview.reset();ime.reset();keyQueue.locked=true;clearInterval(timer);
    status.textContent=isSsh?`SSH terminal paused: ${String(e?.message??e)}. Use Recover output snapshot.`:`Shell stopped: ${String(e?.message??e)}. Close/reopen before continuing.`;
    input.disabled=send.disabled=resize.disabled=true;
    terminalChat.updateOwner();
  }
  finally{if(polling===ticket)polling=null;}
}
function setActiveControls(writable){
  input.disabled=send.disabled=resize.disabled=!writable||restoringLocalLayout;
  close.disabled=detach.disabled=restoringLocalLayout;
  if (writable && fitSize?.checked) void Promise.resolve().then(requestFit);
}
function startLocalTab(targetEpoch,{fromRestore=false,focusTerminal=false,configuration=plainLocalTab}={}) {
  if(sshSession!==null||changingTab||recovery||restoringLocalLayout&&!fromRestore||
      (!owner&&(showLocal.disabled||targetEpoch===null)))return;
  if(owner&&!fromRestore&&(keyQueue.pending||(!keyQueue.locked&&input.disabled))){
    status.textContent='Wait for the current input request to finish before switching shells.';
    return;
  }
  const preserveInput=owner!==null&&!keyQueue.locked&&(fromRestore||!input.disabled);
  changingTab={source:owner,targetEpoch,preserveInput,resizeDisabled:resize.disabled,hadChannel:localStream!==null||localTransport==='recovery',focusTerminal,configuration,
    previousFrame:state,busy:false,view:null};
  if(owner)rememberLocalDraft(owner);
  transport++;polling=null;stopTransport();
  filePreview.dismiss();pasteReview.reset();ime.reset();history.reset();
  setOpenDisabled(true);attach.disabled=button.disabled=true;
  close.disabled=detach.disabled=input.disabled=send.disabled=resize.disabled=true;
  state=null;clearTerminalPaint();
  return attemptLocalTab();
}

async function attemptLocalTab(){
  const change=changingTab;
  if(!change||change.busy)return;
  change.busy=true;renderLocalTabs();
  status.textContent=change.targetEpoch===null?'Opening shell tab…':'Switching shell tabs…';
  try {
    if(!change.view){
      const options=channelSnapshots();
      const launch=change.targetEpoch===null&&change.configuration.shell==='jsh'?{integrated:true}:{};
      if(launch.integrated&&change.configuration.paneId)launch.paneHistoryId=change.configuration.paneId;
      const response=await invoke('activate_local_tab',{owner:change.source,targetEpoch:change.targetEpoch,
        preserveInput:change.preserveInput,...options,...launch});
      if(typeof response?.Err==='string'){
        const previous=change.previousFrame;
        if(previous){paintLiveFrame(previous);state=previous;describeCursor();}
        setActiveControls(change.preserveInput);resize.disabled=change.resizeDisabled;
        if(!owner)close.disabled=detach.disabled=true;
        if(owner)setLocalTools(true);
        else{setOpenDisabled(false);attach.disabled=button.disabled=false;}
        status.textContent=response.Err;
        changingTab=null;renderLocalTabs(true);
        if(owner){
          if(change.hadChannel)await recoverOutput(response.Err);
          else await startTransport();
        }
        return;
      }
      const result=response?.Ok;
      const next=acceptOwner(result?.owner);
      const before=localTabOwners.get(next.epoch);
      if(typeof result.input_allowed!=='boolean'||next.session===change.source?.session||
          (change.targetEpoch===null?next.epoch<=change.source.epoch:next.epoch!==change.targetEpoch)||
          (before&&(next.session!==before.session||next.generation<=before.generation))){
        throw new Error('Invalid local shell tab response');
      }
      change.view={owner:next,frame:acceptAttachedSnapshot(result.frame,next,options),
        writable:result.input_allowed,paneId:acceptLocalPane(result)};
    }
    const next=change.view;
    paintLiveFrame(next.frame);
    await invokeAttached('ack_shell',{epoch:next.owner.epoch,revision:next.frame.revision},next.owner);
    const previousEpoch=epoch;
    owner=next.owner;epoch=owner.epoch;state=next.frame;describeCursor();
    restoreLocalDraft(owner);
    rememberLocalTab(owner,change.targetEpoch===null?change.configuration:undefined,next.paneId);
    updateSplitPair(previousEpoch);rememberLocalGeometry(state);
    detachedLocal=null;
    colsInput.value=String(state.cols);rowsInput.value=String(state.rows.length);
    keyQueue={chain:Promise.resolve(),pending:0,locked:!next.writable};
    localTransport='channel';localInputSuspended=false;
    setActiveControls(next.writable);setLocalTools(true);
    status.textContent=`Local shell ${epoch} active${next.writable?'.':' in view-only mode.'}`;
    changingTab=null;recordLocalLayout();renderLocalTabs(!change.focusTerminal);
    if(change.focusTerminal)(next.writable?imeInput:canvas).focus();
    await startTransport();
  }catch(e){
    if(changingTab!==change)return;
    clearTerminalPaint();
    status.textContent=`Shell tab activation unconfirmed: ${String(e?.message??e)}. Retry the switch to recover the view.`;
    change.busy=false;renderLocalTabs();retryTab.focus();
  }
}
newTab.addEventListener('click',()=>{if(!newTab.disabled)return startLocalTab(null);});
const jshLocalTab={shell:'jsh',presentation:'terminal'};
newJshTab.addEventListener('click',()=>{if(!newJshTab.disabled)return startLocalTab(null,{configuration:jshLocalTab});});
retryTab.addEventListener('click',attemptLocalTab);

async function restoreLocalGeometry(configuration){
  const geometry=configuration.geometry;
  if(!geometry)return; // Historical versions did not record physical sizes.
  const attachment=owner;
  const awaitFrame=async(wanted=null)=>{
    const until=Date.now()+5000;
    while(owner===attachment&&!keyQueue.locked){
      if(localStream?.acknowledging)await localStream.acknowledging;
      if(!localStream)await poll();
      if(state?.session===attachment.session&&state.epoch===attachment.epoch&&!polling&&
          (!wanted||(state.cols===wanted.cols&&state.rows.length===wanted.rows)))return;
      if(Date.now()>=until)break;
      await new Promise(resolve=>setTimeout(resolve,20));
    }
    throw new Error('Restored pane geometry could not be confirmed. Existing shells were retained; no automatic retry.');
  };
  await awaitFrame();
  if(state.cols!==geometry.cols||state.rows.length!==geometry.rows){
    await invokeAttached('resize_shell',{epoch:attachment.epoch,...geometry},attachment);
    await awaitFrame(geometry);
  }
  rememberLocalGeometry(state);
}

restoreLayout.addEventListener('click',async()=>{
  if(retainedLayoutBlocked||restoreLayout.disabled||!savedLayout||owner||localTabEpochs.size||restoringLocalLayout)return;
  const layout=savedLayout;
  splitEnabled=layout.split;splitLayout++;
  restoringLocalLayout=true;
  setOpenDisabled(true);attach.disabled=button.disabled=true;
  status.textContent='Restoring shell tabs…';
  let complete=false;
  try{
    const retained=await listLocalShellEpochs();
    if(retained.length){
      setTabRoster(retained);
      status.textContent='Local shells already exist. Select a retained shell instead.';
      return;
    }
    await openShell({fromRestore:true,configuration:layout.tabs[0]});
    const first=owner;
    if(!first){
      const reason=status.textContent;
      setTabRoster(await listLocalShellEpochs());
      throw new Error(reason);
    }
    if(keyQueue.locked)throw new Error(status.textContent);
    const created=[first.epoch];
    await restoreLocalGeometry(layout.tabs[0]);
    for(const configuration of layout.tabs.slice(1)){
      const previous=owner;
      await startLocalTab(null,{fromRestore:true,configuration});
      if(changingTab)return;
      if(owner===previous||keyQueue.locked)throw new Error(status.textContent);
      created.push(owner.epoch);
      await restoreLocalGeometry(configuration);
    }
    splitEpochs=layout.split?(layout.panes??[0,1]).map(index=>created[index]):[];splitLayout++;
    if(owner.epoch!==created[layout.active]){
      await startLocalTab(created[layout.active],{fromRestore:true});
      if(changingTab)return;
      if(owner.epoch!==created[layout.active]||keyQueue.locked)throw new Error(status.textContent);
    }
    renderSplit();
    if(localStream)await syncObserver(localStream);
    complete=true;
    status.textContent=layout.tabs.length===1?'Opened one fresh shell in the saved layout.':layout.tabs.length===2?'Opened two fresh shells in the saved layout.':`Opened ${layout.tabs.length} fresh shells in the saved layout.`;
  }catch(e){
    status.textContent=`Shell layout restore incomplete. ${String(e?.message??e)}`;
  }finally{
    restoringLocalLayout=false;
    if(!changingTab){
      if(owner){setActiveControls(!keyQueue.locked);setLocalTools(true);}
      else{setOpenDisabled(false);attach.disabled=button.disabled=false;}
    }
    if(complete)recordLocalLayout();
    renderLocalTabs(complete);
  }
});

async function openShell({destination=null,retained=null,fromRestore=false,configuration=plainLocalTab}={}){
  if(epoch!==null||sshSession||restoringLocalLayout&&!fromRestore)return;
  const attempt=++generation;transport++;sshInputSuspended=false;
  filePreview.dismiss();pasteReview.reset();history.reset();setOpenDisabled(true);attach.disabled=button.disabled=true;
  if(destination!==null)status.textContent=`Opening SSH terminal on ${destination}…`;
  const integrated=configuration.shell==='jsh';
  try{
    const options=channelSnapshots();
    const launch=configuration.paneId?{integrated,paneHistoryId:configuration.paneId}:{integrated};
    const result=destination!==null?await invoke('ssh_terminal',{action:{operation:'open',destination}})
      :retained?await invoke('open_second_local_shell',{epoch:retained,...options}):await invoke('open_shell',{...launch,...options});
    if(generation!==attempt)return;
    const paneId=destination===null?acceptLocalPane(result):undefined;
    const opened=destination===null&&paneId!==undefined?acceptOwner({
      session:result.session,epoch:result.epoch,generation:result.generation,
    }):acceptOwner(destination===null?result:result?.owner??result);
    if(retained&&(opened.epoch<=retained||opened.session===detachedLocal?.session||opened.generation!==1))
      throw new Error('Second local shell did not match the open response.');
    owner=opened;sshSession=destination===null?null:opened.session;
    localTransport='channel';localInputSuspended=false;
    if(destination===null){detachedLocal=null;restoreLocalDraft(opened);rememberLocalTab(opened,configuration,paneId);}
    const previousEpoch=epoch;epoch=owner.epoch;updateSplitPair(previousEpoch);keyQueue={chain:Promise.resolve(),pending:0,locked:false};state=null;
    setActiveControls(true);
    if(destination===null){setLocalTools(true);recordLocalLayout();}
    renderLocalTabs();
    status.textContent=retained?'Second local shell opened; the first remains retained.':destination!==null?`SSH shell connected to ${destination}.`:integrated?'Local jsh opened. Completed commands are available in Command blocks.':'Experimental /bin/sh; TERM=dumb. Focus terminal for experimental keyboard input; no native GUI/IME acceptance.';
    if(destination!==null&&typeof result?.save_warning==='string')status.textContent+=' '+result.save_warning;
    await startTransport();
  }catch(e){if(generation===attempt){status.textContent=retained?'Second shell open unconfirmed: '+String(e?.message??e)+'. Show retained local shells before trying again.':String(e?.message??e);if(!fromRestore){setOpenDisabled(false);attach.disabled=button.disabled=false;}}}
}
openSecond.addEventListener('click',()=>{
  if(!openSecond.disabled&&roster.length===1&&!showLocal.disabled)return openShell({retained:roster[0]});
});
attachLocal.addEventListener('click',async()=>{
  const target=selectLocal();
  if(attachLocal.disabled||target===null||showLocal.disabled)return;
  // The target is explicitly selected. Only attach it after Rust confirms
  // retirement of the previous view and its input lease.
  if(owner){
    status.textContent='Switching local shell…';
    if(!(await detachView()))return;
  }
  return reconnect(target);
});
open.addEventListener('click',()=>openShell());
openIntegrated.addEventListener('click',()=>openShell({configuration:jshLocalTab}));
sshForm.addEventListener('submit',event=>{
  event.preventDefault();
  const destination=sshHost.value.trim();
  if(destination&&!openSsh.disabled)return openShell({destination});
});
function selectedSavedSsh(){
  return savedSshShells.find(shell=>shell.id===savedSshSelect.value);
}
function updateSavedSshSelection(){
  resumeSavedSsh.disabled=forgetSavedSsh.disabled=showSavedSsh.disabled||!selectedSavedSsh();
}
async function loadSavedSsh(){
  if(showSavedSsh.disabled)return;
  const request=++savedSshRequest;
  savedSshPanel.hidden=true;savedSshShells=[];updateSavedSshSelection();
  try{
    const list=await invoke('ssh_terminal',{action:{operation:'list_saved'}});
    if(request!==savedSshRequest||showSavedSsh.disabled)return;
    if(!Array.isArray(list)||list.length>16||list.some(shell=>
      !shell||typeof shell.id!=='string'||!/^[a-f0-9]{32}$/.test(shell.id)||
      typeof shell.destination!=='string'||!shell.destination||shell.destination.length>255||
      !Number.isSafeInteger(shell.created_ms)||shell.created_ms<0||shell.created_ms>8640000000000000)||
      new Set(list.map(shell=>shell.id)).size!==list.length)
      throw new Error('Invalid saved SSH shell list.');
    const options=list.map(shell=>{
      const option=document.createElement('option');
      option.value=shell.id;
      option.textContent=shell.destination+' · '+new Date(shell.created_ms).toLocaleString();
      return option;
    });
    savedSshShells=list;
    savedSshSelect.replaceChildren(...options);savedSshSelect.value=list.length===1?list[0].id:'';
    updateSavedSshSelection();savedSshPanel.hidden=!list.length;
    if(!list.length)status.textContent='No saved SSH shells.';
  }catch(error){if(request===savedSshRequest)status.textContent=String(error?.message??error);}
}
showSavedSsh.addEventListener('click',loadSavedSsh);
savedSshSelect.addEventListener('change',updateSavedSshSelection);
resumeSavedSsh.addEventListener('click',()=>{
  const saved=selectedSavedSsh();
  if(saved&&!resumeSavedSsh.disabled)return reconnect(null,saved);
});
forgetSavedSsh.addEventListener('click',async()=>{
  const saved=selectedSavedSsh();
  if(!saved||forgetSavedSsh.disabled)return;
  const request=++savedSshRequest;resumeSavedSsh.disabled=forgetSavedSsh.disabled=true;
  try{
    await invoke('ssh_terminal',{action:{operation:'forget_saved',id:saved.id}});
    if(request===savedSshRequest)await loadSavedSsh();
  }catch(error){if(request===savedSshRequest){status.textContent=String(error?.message??error);updateSavedSshSelection();}}
});
attach.addEventListener('click',()=>reconnect());
async function reconnect(selected=null,saved=null){
  if(epoch!==null||attach.disabled||selected&&sshSession!==null)return;
  const attempt=++generation;filePreview.dismiss();pasteReview.reset();ime.reset();history.reset();
  setOpenDisabled(true);attach.disabled=button.disabled=true;
  const remote=sshSession!==null||saved!==null;
  const expected=!remote&&(!selected||selected===detachedLocal?.epoch)?detachedLocal:null;
  try {
    const options=remote?{}:channelSnapshots();
    const result=saved
      ? await invoke('ssh_terminal',{action:{operation:'restore_saved',id:saved.id}})
      : remote
        ? await invoke('ssh_terminal',{action:{operation:'attach'}})
        : await (selected?invoke('attach_selected_local_shell',{epoch:selected,...options})
          :invoke('attach_shell',expected?{target:{session:expected.session,epoch:expected.epoch},...options}
            :options.frameVersion===undefined?undefined:options));
    if(generation!==attempt)return;
    if(!result||result.ended){
      if(expected)throw new Error('Retained local session no longer available.');
      sshSession=null;status.textContent=result?.message??'No existing shell session.';return;
    }
    const nextOwner=acceptOwner(result.owner);
    if(selected&&nextOwner.epoch!==selected)throw new Error('Selected local shell did not match the reconnect response.');
    if(expected&&(nextOwner.session!==expected.session||nextOwner.epoch!==expected.epoch||
        nextOwner.generation<=expected.generation))throw new Error('Retained local session did not match the reconnect response.');
    const next=acceptAttachedSnapshot(result.frame,nextOwner,options);
    if(saved)sshSession=nextOwner.session;
    if(!remote){detachedLocal=null;restoreLocalDraft(nextOwner);rememberLocalTab(nextOwner);}
    const previousEpoch=epoch;owner=nextOwner;epoch=owner.epoch;updateSplitPair(previousEpoch);keyQueue={chain:Promise.resolve(),pending:0,locked:true};
    localTransport='channel';localInputSuspended=false;
    input.disabled=send.disabled=resize.disabled=true;close.disabled=detach.disabled=false;
    paintLiveFrame(next);state=next;describeCursor();
    await invokeAttached('ack_shell',{epoch:nextOwner.epoch,revision:next.revision},nextOwner);
    if(owner!==nextOwner)return;
    status.textContent=saved?'Reconnected to the saved SSH shell on '+saved.destination+' in view-only mode.'
      :sshSession===null?'Reconnected to the existing shell in view-only mode.':'Reconnected to the SSH shell in view-only mode.';
    if(sshSession===null){
      setLocalTools(true);
      recordLocalLayout();
      renderLocalTabs();
      if(selected!==null)showLocal.focus();
    }
    await startTransport();
  }catch(e){if(generation===attempt)status.textContent=String(e?.message??e);}
  finally{if(generation===attempt&&epoch===null){setOpenDisabled(sshSession!==null);attach.disabled=false;button.disabled=sshSession!==null;}}
}
function clearTerminalPaint(){
  if (liveSelection) liveSelection.reset();
  else clearPaint(canvas);
  canvas.title='';canvas.style.cursor='';
  rememberLocalGeometry(state);
  document.querySelector('#cursor-status').textContent='';
  const health=document.querySelector('#health');health.textContent='';delete health.dataset.transport;
}
async function detachView(){
  if(!owner||detach.disabled||recovery?.epoch===epoch)return false;
  const attempt=++generation,previous=owner;
  // Keep the PTY identity even if the detach reply is lost. A later explicit
  // reconnect must not claim a different retained local terminal.
  if(previous.session!==sshSession){rememberLocalDraft(previous);detachedLocal=previous;}
  owner=null;epoch=null;state=null;transport++;polling=null;
  setLocalTools(false);
  renderLocalTabs();
  stopTransport();sshInputSuspended=false;filePreview.dismiss();pasteReview.reset();ime.reset();history.reset();
  resize.disabled=close.disabled=detach.disabled=input.disabled=send.disabled=true;
  clearTerminalPaint();
  try {
    await invokeAttached('detach_shell',{epoch:previous.epoch},previous);
    if(generation!==attempt)return false;
    status.textContent='View detached. Reconnect to see the running shell.';
    return true;
  }catch(e){
    if(generation===attempt)status.textContent=`Detach unconfirmed: ${String(e?.message??e)}. Reconnect to recover the view.`;
    return false;
  }finally{
    if(generation===attempt){attach.disabled=false;setLocalTools(sshSession===null);renderLocalTabs();}
  }
}
detach.addEventListener('click',detachView);
close.addEventListener('click',async()=>{
  // SSH and local shells can have the same numeric epoch. Retire the poll
  // before Close can give up that view, even if the close response is lost.
  if(changingTab||!owner||close.disabled)return;
  generation++;transport++;polling=null;setLocalTools(false);filePreview.dismiss();pasteReview.reset();ime.reset();history.reset();stopTransport();const old=epoch,oldOwner=owner;epoch=null;owner=null;renderLocalTabs();resize.disabled=close.disabled=detach.disabled=input.disabled=send.disabled=true;
  try{const result=await invokeAttached('close_shell',{epoch:old},oldOwner);if(oldOwner.session!==sshSession){input.value='';localDrafts.delete(oldOwner.session);localTabEpochs.delete(oldOwner.epoch);localTabOwners.delete(oldOwner.epoch);localShellKinds.delete(oldOwner.session);localPaneIds.delete(oldOwner.session);localTabGeometry.delete(oldOwner.epoch);terminalChat.forget(oldOwner.session);if(localTabEpochs.size<2){splitEnabled=false;splitEpochs=[];}else updateSplitPair();recordLocalLayout();}status.textContent=result?.cleanup_pending===false?'Terminal closed; direct child reaped. Descendant containment is not provided.':'Terminal closed; direct-child cleanup handed to bounded supervisor and may still be pending. Descendant containment is not provided.';if(typeof result?.save_warning==='string')status.textContent+=' '+result.save_warning;state=null;clearTerminalPaint();sshSession=null;sshInputSuspended=false;setOpenDisabled(false);attach.disabled=button.disabled=false;renderLocalTabs();}
  catch(e){epoch=old;owner=oldOwner;blocks.updateOwner();terminalChat.updateOwner();close.disabled=detach.disabled=false;status.textContent=`Close error: ${String(e?.message??e)}. Retry Close to finish cleanup.`;}
});
document.querySelector('#command').addEventListener('submit',async event=>{
  event.preventDefault();if(epoch===null||input.disabled)return;
  // Reserve one byte for the appended newline before creating an encoded copy.
  if(input.value.length>4095){status.textContent='Input exceeds 4096-byte bound';return;}
  // jsh's interactive line editor consumes the physical Enter byte.
  const enter=localShellKinds.get(owner?.session)==='jsh'?'\r':'\n';
  const bytes=new TextEncoder().encode(input.value+enter);if(bytes.length>4096){status.textContent='Input exceeds 4096-byte bound';return;}
  send.disabled=true;input.disabled=true;const current=epoch,attachment=owner,queue=keyQueue;let at=0;let failed=false;
  try{await queue.chain;if(!ownsInput(attachment,queue))return;if(queue.locked)throw new Error('input unavailable');for(let attempts=0;at<bytes.length&&attempts<128;attempts++){
    if(!ownsInput(attachment,queue))return;
    if(queue.locked)throw new Error('input became unavailable');
    const n=await invokeAttached('write_shell',{epoch:current,bytes:Array.from(bytes.subarray(at))},attachment);
    // A detached view can reattach at the same epoch. Its old ACK may
    // describe accepted bytes, but cannot edit or refocus the new view.
    if(!ownsInput(attachment,queue))return;
    if(!Number.isInteger(n)||n<0||n>bytes.length-at)throw new Error('invalid write count');at+=n;
    if(n===0)await new Promise(resolve=>setTimeout(resolve,10));
  }if(at!==bytes.length)throw new Error('write blocked; do not resend automatically');input.value='';if(attachment.session!==sshSession)localDrafts.delete(attachment.session);}
  catch(e){failed=true;if(ownsInput(attachment,queue)){pasteReview.reset();queue.locked=true;status.textContent=`${at} bytes accepted: ${String(e?.message??e)}. Input locked; close/reopen before retrying.`;}}
  finally{if(ownsInput(attachment,queue)&&!failed&&!queue.locked){send.disabled=false;input.disabled=false;input.focus();}}
});

const colsInput=document.querySelector('#cols'),rowsInput=document.querySelector('#rows');
colsInput.value ||= '100';rowsInput.value ||= '24';
const sizes=resizeQueue({getEpoch:()=>changingTab||recovery?null:owner,invoke:(command,{epoch:attachment,...args})=>invokeAttached(command,{epoch:attachment.epoch,...args},attachment),onDone:async()=>{await poll();await history.refreshProjection();},onError:e=>{status.textContent=String(e?.message??e);}});
const fitSize = document.querySelector('#fit-size');
const fitViewport = document.querySelector('#active-pane .viewport');
let lastFit = null;
function requestFit() {
  if (!fitSize?.checked || !owner || changingTab || recovery || resize.disabled) return;
  const size = viewportDimensions(fitViewport, globalThis.getComputedStyle?.(fitViewport));
  if (!size || (lastFit?.owner === owner && lastFit.rows === size.rows && lastFit.cols === size.cols)) return;
  lastFit = {owner, ...size};
  if (state?.rows.length === size.rows && state.cols === size.cols) return;
  void sizes.request(size.rows, size.cols);
}
if (fitSize && fitViewport) {
  fitSize.disabled = !invoke;
  fitSize.addEventListener('change', () => {
    lastFit = null;
    fitViewport.classList.toggle('fit-terminal', fitSize.checked);
    if (fitSize.checked) requestFit();
    else sizes.cancelPending();
  });
  if (invoke && typeof ResizeObserver === 'function') new ResizeObserver(requestFit).observe(fitViewport);
}
resize.addEventListener('click', async () => {
  if (epoch === null || input.disabled) return;
  try {
    const {rows, cols} = dimensions(Number(rowsInput.value), Number(colsInput.value));
    if (fitSize?.checked) {
      fitSize.checked = false;
      fitViewport?.classList.remove('fit-terminal');
      lastFit = null;
    }
    await sizes.request(rows, cols);
  } catch (e) {
    status.textContent = String(e?.message ?? e);
  }
});
function describeCursor(frame=visibleLiveFrame()){
  canvas.title='';canvas.style.cursor='';
  if(!frame)return;
  const surface=selectionOverlay?.parentElement ?? canvas;
  const left=`${(surface.offsetLeft||0)+frame.cursor_col*12}px`;
  const top=`${(surface.offsetTop||0)+frame.cursor_row*24}px`;
  for(const layer of [imeInput,terminalPreedit]){
    layer.style.left=left;layer.style.top=top;
  }
  rememberLocalGeometry(state);
  document.querySelector('#cursor-status').textContent=`${frame.cols} × ${frame.rows.length} cells · cursor row ${frame.cursor_row+1}, column ${frame.cursor_col+1} · ${frame.cursor_shape}${frame.cursor_visible?'':' (hidden)'}${frame.pending_wrap?' · wrap pending':''}`;
}
function hyperlinkAt(event){
  const frame=visibleLiveFrame();
  if(!owner||frame?.version!==CHANNEL_FRAME_VERSION||frame.session!==owner.session||frame.epoch!==owner.epoch)return null;
  const point=terminalCellAt(frame,canvas,event.clientX,event.clientY);
  if(!point)return null;
  const cells=frame.rows[point.row];
  const cell=cells[point.col].width===0?cells[point.col-1]:cells[point.col];
  const target=frame.hyperlinks[cell.hyperlink_id];
  return target?{...point,target,revision:frame.revision}:null;
}
canvas.addEventListener('pointermove',event=>{
  const link=hyperlinkAt(event);
  canvas.title=link?`Ctrl-click to open ${link.target}`:'';
  canvas.style.cursor=link&&event.ctrlKey?'pointer':'';
});
canvas.addEventListener('pointerleave',()=>{canvas.title='';canvas.style.cursor='';});
canvas.addEventListener('click',event=>{
  if(!event.isTrusted||event.defaultPrevented||event.button!==0||!event.ctrlKey||event.altKey||event.shiftKey||event.metaKey)return;
  const link=hyperlinkAt(event);
  if(!link)return;
  event.preventDefault();
  const attachment=owner;
  void invoke('open_terminal_hyperlink',{owner:attachment,revision:link.revision,row:link.row,col:link.col}).catch(error=>{
    if(owner===attachment)status.textContent=String(error?.message??error);
  });
},true);
function revealLiveCursor(){
  const frame=visibleLiveFrame();
  if(!frame)return;
  const viewport=document.querySelector('#active-pane .viewport');
  viewport.scrollTo(cursorTarget(frame,selectionOverlay?.parentElement ?? canvas,viewport));
}
document.querySelector('#reveal-cursor').addEventListener('click',revealLiveCursor);


// Browser supplies logical events only; Rust chooses all terminal wire sequences.
let keyQueue={chain:Promise.resolve(),pending:0,locked:false};
function ownsInput(attachment,queue){
  return changingTab===null&&attachment!==null&&owner===attachment&&keyQueue===queue;
}
function enqueueLogical(command,payload,onReceipt){
  if(epoch===null||input.disabled)return;
  const queue=keyQueue;
  if(queue.pending>=128){pasteReview.reset();queue.locked=true;status.textContent='Keyboard queue full; input locked. Close/reopen before retrying.';input.disabled=send.disabled=true;return;}
  const current=epoch,attachment=owner;queue.pending++;
  queue.chain=queue.chain.then(async()=>{
    if(!ownsInput(attachment,queue)||queue.locked)return;
    try{
      const result=await invokeAttached(command,{epoch:current,...payload},attachment);
      if(ownsInput(attachment,queue))onReceipt?.(result);
    }
    catch(e){if(ownsInput(attachment,queue)){pasteReview.reset();queue.locked=true;input.disabled=send.disabled=true;status.textContent=`Input not accepted: ${String(e?.message??e)}. Close/reopen before retrying.`;}}
  }).finally(()=>{queue.pending--;});
  return queue.chain;
}
function keydown(event){
  // Respect an earlier control or capture handler that already owns this key.
  if(event.defaultPrevented)return;
  if(epoch===null||input.disabled||ime.isComposing()||event.isComposing||event.keyCode===229||event.getModifierState?.('AltGraph')||event.metaKey||event.key==='Dead'||event.key==='Process')return;
  if(pasteReview.dismissFromOrigin(event))return;
  if(workflows.consumeEnterRepeat(event)){event.preventDefault();return;}
  if(localView()&&event.ctrlKey&&event.shiftKey&&!event.altKey&&event.key.toLowerCase()==='m'){
    event.preventDefault();
    if(!event.repeat)workflows.toggle();
    return;
  }
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

copySelection?.addEventListener('click', () => {
  const selection = liveSelection?.snapshot();
  if (!selection) return;
  history.reset();
  const output = document.querySelector('#history-output');
  output.value = selection.text;
  output.focus();
  output.select();
});

document.querySelector('#copy-visible').addEventListener('click',()=>{
  const frame = visibleLiveFrame();
  if(!frame)return;
  history.reset();
  const output=document.querySelector('#history-output');
  // Match Rust archive copy: strip ASCII blank padding, not displayed
  // Unicode spaces such as NBSP or the wide ideographic space.
  output.value=frame.rows.map((row,index)=>{
    const text=row.filter(cell=>cell.width!==0).map(cell=>cell.text).join('');
    return row.wrapped?text:text.replace(/ +$/,'')+(index+1<frame.rows.length?'\n':'');
  }).join('');
  document.querySelector('#history-label').textContent=`Visible grid snapshot ${frame.revision}. Select and copy normally.`;
});

async function recoverOutput(previousStatus=null){
  if(epoch===null||changingTab||recovery?.epoch===epoch)return;
  const current=epoch,attachment=owner,ticket=++transport,operation={epoch:current};recovery=operation;detach.disabled=true;stopTransport();polling=null;
  ime.reset();
  setLocalTools(false);renderLocalTabs();
  try {
    if(attachment.session===sshSession&&sshReloadNeeded){
      sshReloadNeeded=false;
      const result=await invoke('ssh_terminal',{action:{operation:'prepare_reload',owner:attachment,view:captureSshReloadView()}});
      if(epoch!==current||transport!==ticket)return;
      saveReloadTicket(result?.ticket,globalThis.sessionStorage);
      globalThis.location.reload();
      return;
    }
    const isLocal=attachment.session!==sshSession;
    let options=isLocal&&(localTransport==='channel'||localTransport==='recovery'&&localFrameRecovery)?channelSnapshots():{};
    let result;
    try{result=await invokeAttached('recover_shell',{epoch:current,...options},attachment);}
    catch(error){
      // Encoding precedes the host's generation change. The v3 path can be
      // retried with the Channel when its smaller counters cannot name the snapshot.
      if(!isLocal||localTransport!=='recovery'||options.frameVersion||typeof Channel!=='function'||
          !['revision exhausted','layout exhausted'].includes(String(error?.message??error))||
          epoch!==current||transport!==ticket)throw error;
      options=channelSnapshots();
      result=await invokeAttached('recover_shell',{epoch:current,...options},attachment);
    }
    if(epoch!==current||transport!==ticket)return;
    const nextOwner=acceptOwner(result.owner);
    if(nextOwner.session!==attachment.session||nextOwner.epoch!==current||
        nextOwner.generation<=attachment.generation) throw new Error('foreign terminal recovery');
    const next=acceptAttachedSnapshot(result.frame,nextOwner,options);
    if(keyQueue.pending||(!keyQueue.locked&&input.disabled&&!restoringLocalLayout)){
      pasteReview.reset();keyQueue.locked=true;input.disabled=send.disabled=true;
    }
    owner=nextOwner;
    if(nextOwner.session!==sshSession)rememberLocalTab(nextOwner);
    paintLiveFrame(next);state=next;describeCursor();
    await invokeAttached('ack_shell',{epoch:current,revision:next.revision});
    if(epoch!==current||transport!==ticket)return;
    if(nextOwner.session===sshSession){
      if(result.input_allowed!==true){pasteReview.reset();keyQueue.locked=true;input.disabled=send.disabled=resize.disabled=true;}
      else if(sshInputSuspended&&!keyQueue.pending){keyQueue.locked=false;input.disabled=send.disabled=resize.disabled=false;}
      sshInputSuspended=false;
      status.textContent=result.input_allowed===true?'SSH output snapshot recovered.':'SSH output snapshot recovered. Input remains disabled.';
    }else if(localTransport==='recovery'){
      localTransport=options.frameVersion===CHANNEL_FRAME_VERSION?'channel':'poll';
      localFrameRecovery=false;
      if(localInputSuspended&&result.input_allowed===true&&!keyQueue.pending){
        keyQueue.locked=false;input.disabled=send.disabled=resize.disabled=restoringLocalLayout;
      }
      localInputSuspended=false;status.textContent=localTransport==='poll'
        ?'Local output recovered with compatibility polling.':'Local output snapshot recovered.';
    }else{status.textContent=previousStatus??'Output snapshot recovered. Existing input safety locks remain unchanged.';}
    await startTransport();
  }catch(e){if(epoch===current&&transport===ticket)status.textContent=`Recovery failed: ${String(e?.message??e)}. Retry recovery or close.`;}
  finally{if(recovery===operation){recovery=null;if(epoch===current&&transport===ticket){detach.disabled=false;setLocalTools(true);renderLocalTabs();}}}
}
document.querySelector('#recover').addEventListener('click',()=>recoverOutput());


function captureSshReloadView(){
  return validateReloadView({version:1,tabs:[...localTabEpochs].sort((a,b)=>a-b).map(number=>{
    const retained=localTabOwners.get(number);
    return {owner:retained,shell:localShellKinds.get(retained?.session),paneId:localPaneIds.get(retained?.session)??null,
      geometry:localTabGeometry.get(number)??null,draft:localDrafts.get(retained?.session)??''};
  }),splitEnabled,splitEpochs:splitEnabled?[...splitEpochs]:[],detachedLocal,remoteDraft:input.value});
}
function restoreSshReloadView(view,{remoteDraft=true}={}){
  // Rehydrate identities only. The saved-layout launch action is intentionally
  // never called: all of these terminals already exist in the native runtime.
  for(const tab of view.tabs){
    localTabEpochs.add(tab.owner.epoch);localTabOwners.set(tab.owner.epoch,tab.owner);
    localShellKinds.set(tab.owner.session,tab.shell);
    if(tab.paneId!==null)localPaneIds.set(tab.owner.session,tab.paneId);
    if(tab.geometry!==null)localTabGeometry.set(tab.owner.epoch,tab.geometry);
    if(tab.draft)localDrafts.set(tab.owner.session,tab.draft);
  }
  splitEnabled=view.splitEnabled;splitEpochs=view.splitEpochs;detachedLocal=view.detachedLocal;
  if(remoteDraft)input.value=view.remoteDraft;
}
async function claimSshReload(){
  let ticket,grant=false;
  try{ticket=takeReloadTicket(globalThis.sessionStorage);}catch(error){await recoverSshReloadLayout(error);return;}
  if(!invoke)return;
  if(!ticket){await recoverSshReloadLayout();return;}
  setOpenDisabled(true);attach.disabled=button.disabled=true;
  try{
    const result=await invoke('ssh_terminal',{action:{operation:'claim_reload',ticket}});
    const view=validateReloadView(result?.view),nextOwner=acceptOwner(result?.owner);
    if(typeof result.input_allowed!=='boolean')throw new Error('Missing SSH host input grant.');
    grant=result.input_allowed;
    const next=acceptFull(null,result.frame,nextOwner);
    restoreSshReloadView(view);
    owner=nextOwner;epoch=nextOwner.epoch;sshSession=nextOwner.session;state=next;
    keyQueue={chain:Promise.resolve(),pending:0,locked:true};input.disabled=send.disabled=resize.disabled=true;
    close.disabled=detach.disabled=false;
    paintLiveFrame(next);describeCursor();renderLocalTabs();
    await invokeAttached('ack_shell',{epoch:nextOwner.epoch,revision:next.revision},nextOwner);
    if(owner!==nextOwner)return;
    if(result.input_allowed){keyQueue.locked=false;input.disabled=send.disabled=resize.disabled=false;}
    status.textContent=result.input_allowed?'SSH output recovered in a fresh input view.':'SSH view recovered. Input remains disabled.';
    await startTransport();
  }catch(error){
    keyQueue.locked=true;input.disabled=send.disabled=resize.disabled=true;
    if(owner?.session===sshSession&&grant&&!keyQueue.pending)sshInputSuspended=true;
    status.textContent=`SSH input view reset failed: ${String(error?.message??error)}. Reconnect view-only; do not resend pending input.`;
    if(owner===null)await recoverSshReloadLayout(error);
  }
}

async function recoverSshReloadLayout(reason=null){
  if(!invoke)return;
  const attempt=generation,previous=owner;let relevant=!!reason;
  if(reason){retainedLayoutBlocked=true;keyQueue.locked=true;input.disabled=send.disabled=resize.disabled=true;}
  try{
    const result=await invoke('ssh_terminal',{action:{operation:'recover_reload_layout'}});
    if(generation!==attempt||owner!==previous)return;
    if(!result&&!reason)return;
    relevant=true;
    const retained=await listLocalShellEpochs();
    if(generation!==attempt||owner!==previous)return;
    retainedLayoutBlocked=retained.length>0;
    if(result?.view){
      const view=retainedReloadView(result.view,retained);
      restoreSshReloadView(view,{remoteDraft:false});
    }
    setTabRoster(retained);
    keyQueue.locked=true;input.disabled=send.disabled=resize.disabled=true;
    status.textContent='SSH writer recovery was not completed. Retained local panes are available view-only; use Saved SSH shells for a reader. Unsent remote input was not restored.';
  }catch(error){
    if(generation!==attempt||owner!==previous)return;
    relevant=true;retainedLayoutBlocked=true;
    status.textContent='SSH writer recovery is locked. List retained local shells or reconnect Saved SSH view-only. No saved-layout shells were launched.';
  }finally{
    if(relevant&&generation===attempt&&owner===previous){
      if(owner===null){setOpenDisabled(false);attach.disabled=false;button.disabled=false;}
      renderLocalTabs();
    }
  }
}
void claimSshReload();
