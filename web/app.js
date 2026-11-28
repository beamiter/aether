import { installTerminalKeys, modifierKey } from './terminal-keys.js';
import {paneInputIntent} from './pane-input-intent.js';
import {directionalPaneNeighbor,retainedPaneTree} from './pane-geometry.js';
import {makePassiveSurfaces} from './passive-surfaces.js';
import {installTerminalMouse} from './terminal-mouse.js';
import {installTerminalFocus} from './terminal-focus.js';
import {installTerminalSelection} from './terminal-selection.js';
import {installTerminalShell} from './terminal-shell.js';
import {validateReloadView,retainedReloadView,saveReloadTicket,takeReloadTicket} from './ssh-reload.js';
import {installFilePreviewPanel} from './file-preview-panel.js';
import {openVim9Snapshot} from './vim9-workbench.js';
import {installPasteReview} from './paste.js';
import {dimensions,resizeQueue,cursorTarget,terminalCellAt,viewportDimensions,windowPixelQueue} from './viewport.js';
import {installHistory} from './history.js';
import {installInlineHistory} from './inline-history.js';
import {installCommandBlocks} from './command-blocks.js';
import {installTerminalChat,validateChatStatus} from './terminal-chat.js';
import {installWorkflows} from './workflows.js';
import {plainLocalTab,readLocalShellLayout,saveLocalShellLayout,readThreePaneLayout,saveThreePaneLayout,validPaneId,MAX_LOCAL_SHELLS,validLocalGeometry,localSplitPair} from './local-layout.js';
import {backingGeometry,acceptFull,acceptUpdate,clearPaint,paint} from './frames.js'; import {installIme} from './ime.js';
import {acceptChannelFrame,acceptChannelPayload,acceptChannelUpdate,channelCounter,CHANNEL_DENSE_VERSION,decodeLocalChannelEvent,supportsChannelRendition} from './frames-v4.js';
const canvas=document.querySelector('canvas'),status=document.querySelector('#status'),button=document.querySelector('#demo');
const open=document.querySelector('#open'),attach=document.querySelector('#attach'),detach=document.querySelector('#detach'),close=document.querySelector('#close'),input=document.querySelector('#input'),send=document.querySelector('#send');
const openIntegrated=document.querySelector('#open-integrated');
const sshForm=document.querySelector('#ssh-terminal-form'),sshHost=document.querySelector('#ssh-terminal-host'),openSsh=document.querySelector('#open-ssh');
const localTools=document.querySelector('#local-shells'),showLocal=document.querySelector('#manage-local'),localOptions=document.querySelector('#local-shell-options'),localSelection=document.querySelector('#local-shell-selection'),attachLocal=document.querySelector('#attach-selected-local'),openSecond=document.querySelector('#open-second-local');
const tabs=document.querySelector('#local-tabs'),tabList=document.querySelector('#local-tab-list'),newTab=document.querySelector('#new-local-tab'),retryTab=document.querySelector('#retry-local-tab');
const newJshTab=document.querySelector('#new-jsh-tab');
const splitToggle=document.querySelector('#split-local'),panes=document.querySelector('#terminal-panes'),peerPane=document.querySelector('#peer-pane'),peerCanvas=document.querySelector('#peer-canvas'),focusPeer=document.querySelector('#focus-peer-pane');
const maximizePane=document.querySelector('#maximize-active-pane');
const splitBalanceControl=document.querySelector('#split-balance-control');
const splitBalanceLabel=document.querySelector('#split-balance-label');
const splitBalance=document.querySelector('#split-balance');
const splitStackControl=document.querySelector('#split-stack-control');
const splitStackBalance=document.querySelector('#split-stack-balance');

const tripleToggle=document.querySelector('#split-three-local'),activePane=document.querySelector('#active-pane');
const passivePanes=[1,2].map(index=>({
  section:document.querySelector(`#preview-pane-${index}`),
  canvas:document.querySelector(`#preview-canvas-${index}`),
  focus:document.querySelector(`#focus-preview-pane-${index}`),
}));
const restoreLayout=document.querySelector('#restore-local-layout'),saveRecoveredLayout=document.querySelector('#save-recovered-layout');
const terminalPanel=document.querySelector('#terminal-panel');
const emptyTerminal=document.querySelector('#terminal-empty');
const sessionIndicator=document.querySelector('#session-indicator');
const sessionState=document.querySelector('#session-state');
const terminalSize=document.querySelector('#terminal-size');
const sshConnect=document.querySelector('#show-ssh-connection');
const recoverButton=document.querySelector('#recover');
let roster=[],localListRequest=0;
const localDrafts=new Map();
const localTabEpochs=new Set();
const localTabOwners=new Map();
const localShellKinds=new Map();
const localPaneIds=new Map();
const localTabGeometry=new Map();
let changingTab=null,restoringLocalLayout=false;
let mouseView=null,hoveredLinkPoint=null;
let state=null,epoch=null,owner=null,timer=null,polling=null,generation=0,transport=0,recovery=null;
let sshSession=null,sshInputSuspended=false,detachedLocal=null,sshReloadNeeded=false,retainedLayoutBlocked=false;
let sshFocusOwner=null,sshMouseOwner=null,sshKeyboardOwner=null;
let sshPoll=null;
const showSavedSsh=document.querySelector('#show-saved-ssh'),savedSshPanel=document.querySelector('#saved-ssh');
const savedSshSelect=document.querySelector('#saved-ssh-shell'),resumeSavedSsh=document.querySelector('#resume-saved-ssh'),forgetSavedSsh=document.querySelector('#forget-saved-ssh');
let savedSshShells=[],savedSshRequest=0;
let localStream=null,localTransport='channel',localInputSuspended=false,localFrameRecovery=false;
let maximizedPane=null;
let splitEnabled=false,splitEpochs=[],splitLayout=1,layoutSerial=1,settingSplit=false,peerState=null;
let tripleEpochs=[],passiveTargets=[null,null];
function allocateLayoutToken(){
  if(layoutSerial===0xffffffff)throw new Error('Terminal layout tokens exhausted');
  return ++layoutSerial;
}
const passiveSurfaces=makePassiveSurfaces({
  allocate:allocateLayoutToken,
  clear(surface){clearPaint(surface);delete surface.dataset.geometry;},
  paint(surface,frame){
    paint(surface,frame);
    surface.dataset.geometry=`${frame.cols}x${frame.rows.length}`;
    rememberLocalGeometry(frame);
  },
});
function readPreference(key){
  try{return globalThis.localStorage?.getItem(key)??null;}catch{return null;}
}
function writePreference(key,value){
  try{globalThis.localStorage?.setItem(key,value);}catch{/* A preference must not interrupt the terminal. */}
}
// Keep the initial three-surface journey opt-in until the desktop checks
// prove isolation and the saved recursive layout is ready.
const threePanePreview=readPreference('aether.preview-three-panes')==='1';
let splitAxis=readPreference('aether.split-axis')==='vertical'?'vertical':'horizontal';
const splitShareKey=axis=>axis==='vertical'?'aether.split-share.rows.v1':'aether.split-share.columns.v1';
const tripleShareKeys={columns:'aether.three-pane-share.columns.v1',rows:'aether.three-pane-share.rows.v1'};
// Keep numeric preferences out of the saved shell roster and preview layout.
// Older builds can ignore these keys without losing their saved PTY layout.
const splitShareValue=value=>/^(?:[1-8][0-9]|90)$/.test(value??'')?Number(value):null;
const splitShares={
  horizontal:splitShareValue(readPreference(splitShareKey('horizontal')))??50,
  vertical:splitShareValue(readPreference(splitShareKey('vertical')))??50,
};
const tripleShares={
  columns:splitShareValue(readPreference(tripleShareKeys.columns))??50,
  rows:splitShareValue(readPreference(tripleShareKeys.rows))??50,
};
const selectionOverlay = document.querySelector('#terminal-selection');
const copySelection = document.querySelector('#copy-selection');
const liveSelection = selectionOverlay && typeof canvas.setPointerCapture === 'function'
  ? installTerminalSelection({
    canvas, overlay: selectionOverlay, document,
    joinWrappedRows: frame => frame.session !== sshSession,
    onFrame(frame) {
      if (frame) paintTerminalFrame(frame);
      else clearPaint(canvas);
      describeCursor(frame);
    },
    onSelection(available) { if (copySelection) copySelection.disabled = !available; },
  })
  : null;
function paintLiveFrame(frame) {
  if (liveSelection) liveSelection.present(frame);
  else paintTerminalFrame(frame);
}
const visibleLiveFrame = () => liveSelection?.visibleFrame() ?? state;
const filePreview=installFilePreviewPanel(document,{getFrame:visibleLiveFrame,getEpoch:()=>changingTab?null:epoch,getSession:()=>changingTab?null:owner?.session,
  getOwner:()=>!changingTab&&!recovery&&owner?JSON.stringify([owner.session,owner.epoch,owner.generation]):null,isLocalTerminal:()=>!sshSession,
  openInVim:typeof openVim9Snapshot==='function'?openVim9Snapshot:null});
const imeInput=document.querySelector('#ime-input');
const terminalPreedit=document.querySelector('#terminal-preedit');
const historyCanvas=document.createElement('canvas');
historyCanvas.setAttribute('aria-label','Codex terminal history');
Object.assign(historyCanvas.style,{position:'absolute',top:'0',left:'0',zIndex:'1'});
canvas.parentElement?.append(historyCanvas);
const inlineHistory=installInlineHistory({
  canvas:historyCanvas,viewport:document.querySelector('#active-pane .viewport'),
  getOwner:()=>changingTab||recovery?null:owner,getFrame:visibleLiveFrame,
  fetchHistory:(attachment,options)=>invokeAttached('history_shell',{epoch:attachment.epoch,...options},attachment),
  focus:()=>imeInput.focus(),
});
function matchesMouseCanvas(mouse) {
  return !mouse.pixels || (canvas.width === mouse.pixels.width && canvas.height === mouse.pixels.height);
}
const terminalMouse = typeof canvas.setPointerCapture === 'function'
  ? installTerminalMouse({
    canvas,
    getFrame() {
      if (input.disabled || changingTab || recovery || inlineHistory.active() || !canReportMouse(owner) ||
          mouseView?.owner !== owner || !matchesMouseCanvas(mouseView.mouse)) return null;
      const frame = visibleLiveFrame();
      if (!frame || frame.session !== owner.session || frame.epoch !== owner.epoch ||
          String(frame.revision) !== mouseView.mouse.revision ||
          String(frame.layout) !== mouseView.mouse.layout) return null;
      return {...frame, mouse: mouseView.mouse, attachment: owner};
    },
    canRelease(frame) {
      return !input.disabled && !changingTab && !recovery &&
        canReportMouse(owner) && frame.attachment === owner && matchesMouseCanvas(frame.mouse);
    },
    send: report => enqueueLogical('mouse_shell', {report}),
    focus: () => imeInput.focus({preventScroll:true}),
  })
  : null;
const terminalChatSend=document.querySelector('#terminal-chat-send');
const inputOwner=()=>!input.disabled?owner:null;
function refreshTerminalControls(){terminalChat.updateOwner();workflows.updateOwner();}
function sendTerminalText(text,attachment){
  if(attachment!==owner)return;
  if(text.length>4096||new TextEncoder().encode(text).length>4096){
    status.textContent='Committed text exceeds 4096-byte limit; nothing sent.';return;
  }
  enqueueLogical('text_shell',{text});
}
const terminalKeys=installTerminalKeys({
  getOwner:()=>keyboardView()===inputOwner()?inputOwner():null,
  send:(event,receipt,permit)=>enqueueLogical('key_event_shell',{event},receipt,permit),
  sendText:sendTerminalText,
});
const ime=installIme(imeInput,{getEpoch:inputOwner,
canCommit:()=>!document.hidden&&(document.activeElement??imeInput)===imeInput,
afterEnd:callback=>{
  const flush=()=>{callback();refreshTerminalControls();};
  if(typeof requestAnimationFrame==='function')requestAnimationFrame(flush);else flush();
},preedit:text=>{
  terminalPreedit.textContent=text;terminalPreedit.hidden=text==='';
  void Promise.resolve().then(refreshTerminalControls);
},onCompose:terminalKeys.clearText,typed:terminalKeys.text,commit:sendTerminalText});
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
globalThis.addEventListener?.('blur',()=>{ime.reset();terminalKeys.reset();splitIntent.cancel();});
imeInput.addEventListener('blur',terminalKeys.reset);
canvas.addEventListener('blur',terminalKeys.reset);
document.addEventListener?.('visibilitychange',()=>{if(document.hidden){terminalKeys.reset();splitIntent.cancel();}});
canvas.addEventListener('click',event=>{if(!event.defaultPrevented&&!event.ctrlKey)imeInput.focus();});
const resize=document.querySelector('#resize');
const invoke=globalThis.__TAURI__?.core?.invoke;
const Channel=globalThis.__TAURI__?.core?.Channel;
function windowPixelOwner() {
  return sshSession === null && !changingTab && !recovery && !keyQueue.locked ? owner : null;
}
const reportWindowPixels = windowPixelQueue({
  getOwner: windowPixelOwner,
  update: (attachment, geometry) => invoke('set_shell_window_pixels', {owner:attachment, ...geometry}),
});
let lastPaintScale = null;
function reportPaintedWindowPixels(frame) {
  const active = windowPixelOwner();
  if (!invoke || !frame || !active ||
      frame.session !== active.session || frame.epoch !== active.epoch) return;
  if (canvas.width > 0 && canvas.height > 0) {
    reportWindowPixels(active, String(frame.layout), frame.rows.length, frame.cols, canvas.width, canvas.height);
  }
}
function paintTerminalFrame(frame) {
  lastPaintScale = globalThis.devicePixelRatio || 1;
  paint(canvas, frame, lastPaintScale);
  reportPaintedWindowPixels(frame);
}
function windowPixels(rows, cols) {
  const {width, height} = backingGeometry(rows, cols);
  return {width, height};
}
// The local owner opens a 24 by 80 grid before any viewport fitting.
function initialWindowPixels() { return windowPixels(24, 80); }


const channelSnapshots=()=>typeof Channel==='function'?{frameVersion:CHANNEL_DENSE_VERSION}:{};
const acceptAttachedSnapshot=(frame,attachment,options)=>options.frameVersion
  ?acceptChannelFrame(null,frame,{...attachment,frameVersion:options.frameVersion},true):acceptFull(null,frame,attachment);
let savedLayout=invoke?(threePanePreview?readThreePaneLayout():null)??readLocalShellLayout():null,protectSavedLayout=false;
const sshCommands={poll_shell:'poll',poll_ssh_shell:'poll_with_status',poll_packed_shell:'poll_packed',ack_shell:'ack',shell_status:'status',history_shell:'history',recover_shell:'recover',detach_shell:'detach',close_shell:'close',write_shell:'write',key_shell:'key',key_event_shell:'key_event',text_shell:'text',paste_shell:'paste',resize_shell:'resize',focus_shell:'focus',mouse_shell:'mouse'};
function sshPollFor(attachment,info){
  if(info?.poll_with_status!==true)return null;
  const version=info.packed_frame_version;
  const frameVersion=supportsChannelRendition(version)?version:null;
  return {owner:attachment,ackRevision:null,frameVersion};
}
function acceptSshSnapshot(result,attachment){
  if(result.frame_version===CHANNEL_DENSE_VERSION){
    return acceptChannelUpdate(null,result.frame,{...attachment,frameVersion:CHANNEL_DENSE_VERSION},
      true,result.frame_bytes);
  }
  return acceptFull(null,result.frame,attachment);
}
async function confirmSnapshot(attachment,frame,resumeInput=false){
  if(attachment.session!==sshSession||frame.version!==CHANNEL_DENSE_VERSION){
    await invokeAttached('ack_shell',{epoch:attachment.epoch,revision:frame.revision},attachment);
    return {polling:false};
  }
  sshPoll={owner:attachment,ackRevision:frame.revision,frameVersion:frame.version};
  keyQueue.locked=true;
  input.disabled=send.disabled=resize.disabled=true;
  const confirmed=await startTransport(info=>{
    if(resumeInput&&!keyQueue.pending&&!info.exited&&!info.drained&&info.write_error===null){
      keyQueue.locked=false;
      input.disabled=send.disabled=resize.disabled=false;
    }
  });
  if(!confirmed){
    if(owner===attachment&&resumeInput&&!keyQueue.pending)sshInputSuspended=true;
    return null;
  }
  return {polling:true};
}
function setOpenDisabled(disabled) {
  open.disabled=openIntegrated.disabled=openSsh.disabled=sshHost.disabled=disabled||!invoke;
  sshConnect.disabled=openSsh.disabled;
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
function canSaveRecoveredLayout(){
  return protectSavedLayout&&!!owner&&sshSession===null&&!restoringLocalLayout&&!changingTab&&!recovery&&
    !settingSplit&&!tripleEpochs.length&&localTabEpochs.has(epoch)&&
    [...localTabEpochs].every(value=>localTabOwners.has(value)&&localTabGeometry.has(value));
}
function renderSavedLayout(){
  saveRecoveredLayout.hidden=!protectSavedLayout||!owner||sshSession!==null;
  saveRecoveredLayout.disabled=!canSaveRecoveredLayout();
  restoreLayout.hidden=retainedLayoutBlocked||!savedLayout||owner!==null||sshSession!==null||localTabEpochs.size>0;
  restoreLayout.disabled=retainedLayoutBlocked||open.disabled||restoringLocalLayout;
}
function renderTerminalStage(){
  if(inPlaceHistory&&(historyOrigin!==owner||changingTab||recovery))closeHistory(false);
  const phase=owner?(keyQueue.locked?'read-only':'live'):state?'preview':'idle';
  sessionIndicator.dataset.state=phase;
  const title=phase==='idle'?'No session':phase==='preview'?'Frame preview':
    (owner.session===sshSession?'SSH':'Local')+(phase==='read-only'?' · Read only':' · Live');
  if(sessionState.textContent!==title)sessionState.textContent=title;
  emptyTerminal.hidden=!!(owner||state||changingTab);
  canvas.tabIndex=emptyTerminal.hidden?0:-1;
  open.hidden=!!owner||!!changingTab;
  newTab.hidden=newJshTab.hidden=!owner||sshSession!==null;
  recoverButton.disabled=!owner||!!changingTab||!!recovery;
  const size=state?`${state.cols} × ${state.rows.length}`:'';
  if(terminalSize.textContent!==size)terminalSize.textContent=size;
}
function focusTerminal(){
  if(terminalPanel.hidden)return;
  document.body.scrollTop=0;
  (emptyTerminal.hidden?(input.disabled?canvas:imeInput):open).focus({preventScroll:true});
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
  if(tripleEpochs.length){
    if(!tripleEpochs.every(value=>localTabEpochs.has(value)))tripleEpochs=[];
    else if(localTabEpochs.has(epoch)&&!tripleEpochs.includes(epoch)){
      const slot=tripleEpochs.indexOf(previous);
      if(slot<0)tripleEpochs=[];
      else tripleEpochs=tripleEpochs.map((value,index)=>index===slot?epoch:value);
    }
  }
  if(splitEnabled)splitEpochs=localSplitPair([...localTabEpochs].sort((a,b)=>a-b),epoch,splitEpochs,previous);
}
function recordLocalLayout(){
  if(protectSavedLayout){renderSavedLayout();return;}
  persistLocalLayout(false);
}
function persistLocalLayout(deliberate){
  if(restoringLocalLayout||changingTab||settingSplit)return false;
  const epochs=[...localTabEpochs].sort((a,b)=>a-b);
  const configurations=[];
  for(const number of epochs){
    const session=localTabOwners.get(number)?.session;
    const shell=localShellKinds.get(session);
    if(!shell)return false;
    configurations.push({shell,presentation:terminalChat.modeFor(session),paneId:localPaneIds.get(session)});
  }
  const active=Math.max(0,epochs.indexOf(epoch));
  const split=splitEnabled&&splitEpochs.length===2&&splitEpochs.every(number=>epochs.includes(number));
  const complete=epochs.every(number=>localTabGeometry.has(number));
  if((epochs.length>2||deliberate)&&!complete)return false;
  const next=epochs.length?{tabs:configurations,active,split}:null;
  if(next&&complete){
    next.tabs=configurations.map((tab,index)=>({...tab,geometry:localTabGeometry.get(epochs[index])}));
    next.panes=split?splitEpochs.map(number=>epochs.indexOf(number)):[];
  }
  if(threePanePreview&&tripleEpochs.length){
    if(deliberate||!complete||!epochs.includes(epoch))return false;
    const triple=tripleEpochs.map(number=>epochs.indexOf(number));
    if(triple.some(index=>index<0))return false;
    const preview={tabs:next.tabs,active,triple};
    if(!saveThreePaneLayout(preview))return false;
    savedLayout=preview;renderSavedLayout();return true;
  }
  const stored=saveLocalShellLayout(next)&&(!threePanePreview||saveThreePaneLayout(null));
  if(deliberate&&!stored)return false;
  savedLayout=next;
  if(deliberate)protectSavedLayout=false;
  renderSavedLayout();
  return stored;
}
saveRecoveredLayout.addEventListener('click',()=>{
  if(!canSaveRecoveredLayout())return;
  status.textContent=persistLocalLayout(true)?'Saved current shell tabs.':'Could not save current tabs. The previous layout is retained.';
});
function observerTarget(){
  if(tripleIsVisible()||!splitEnabled||!owner||sshSession!==null||localTabEpochs.size<2||!splitEpochs.includes(epoch))return null;
  const other=splitEpochs.find(number=>number!==epoch&&localTabEpochs.has(number));
  return other===undefined?null:{epoch:other,layout:splitLayout};
}
function tripleIsVisible(){
  return threePanePreview&&!!owner&&sshSession===null&&localTransport==='channel'&&tripleEpochs.length===3&&
    tripleEpochs.includes(epoch)&&tripleEpochs.every(value=>localTabOwners.has(value));
}
function knownLocalEpochs(){
  return [...localTabEpochs].filter(value=>localTabOwners.has(value)).sort((a,b)=>a-b);
}
function renderTriple(disabled){
  const visible=tripleIsVisible();
  tripleToggle.hidden=!threePanePreview||sshSession!==null||knownLocalEpochs().length<3;
  tripleToggle.disabled=disabled||tripleToggle.hidden||!localStream?.ready;
  tripleToggle.setAttribute('aria-pressed',String(visible));
  tripleToggle.textContent=visible?'Leave three panes':'Three panes';
  panes.dataset.triple=visible?'true':'';
  activePane.dataset.slot=visible?String(tripleEpochs.indexOf(epoch)):'';
  const peers=visible?tripleEpochs.flatMap((value,slot)=>value===epoch?[]:[{value,slot}]):[];
  passiveTargets=passiveSurfaces.reconcile(peers.map(({value,slot},index)=>({
    leaf:`live-slot-${slot}`,epoch:value,session:localTabOwners.get(value).session,canvas:passivePanes[index].canvas,
  })));
  passivePanes.forEach(({section,focus},index)=>{
    const peer=peers[index];
    section.hidden=!peer;
    section.dataset.slot=peer?String(peer.slot):'';
    focus.disabled=disabled||!peer;
    if(peer)focus.textContent=`Focus Shell ${peer.value}`;
  });
}
function renderPaneMaximize(hasPeers){
  if(!hasPeers||owner!==maximizedPane?.owner||splitLayout!==maximizedPane?.layout||changingTab||recovery)
    maximizedPane=null;
  const zoomed=maximizedPane!==null;
  panes.dataset.maximized=zoomed?'true':'';
  maximizePane.hidden=!hasPeers;
  maximizePane.disabled=!hasPeers||splitToggle.disabled;
  maximizePane.setAttribute('aria-pressed',String(zoomed));
  maximizePane.textContent=zoomed?'Restore split':'Maximize pane';
}
function togglePaneMaximize(){
  if(maximizePane.disabled||maximizePane.hidden)return;
  // The live tree and passive observers stay in place. Changing the
  // writer or replacing the layout automatically leaves the zoomed view.
  maximizedPane=maximizedPane?null:{owner,layout:splitLayout};
  renderSplit();
  commandPalette.refreshAvailability();
}
maximizePane.addEventListener('click',togglePaneMaximize);
function renderSplit(){
  const target=observerTarget();
  splitToggle.disabled=!owner||sshSession!==null||!!changingTab||!!recovery||restoringLocalLayout||settingSplit;
  renderTriple(splitToggle.disabled);
  splitToggle.setAttribute('aria-pressed',String(!!target||tripleIsVisible()));
  splitToggle.textContent=(target||tripleIsVisible())?'Single pane':'Split';
  const first=splitEpochs[0]===epoch;
  panes.dataset.split=target?(splitAxis==='vertical'?(first?'top':'bottom'):(first?'left':'right')):'';
  peerPane.hidden=!target;
  const triple=tripleIsVisible();
  renderPaneMaximize(Boolean(target||triple));
  splitBalanceControl.hidden=!!maximizedPane||(!target&&!triple);
  splitBalance.disabled=splitBalanceControl.hidden||splitToggle.disabled;
  splitStackControl.hidden=!!maximizedPane||!triple;
  splitStackBalance.disabled=splitStackControl.hidden||splitToggle.disabled;
  if(target||triple)renderSplitBalance(triple);
  focusPeer.disabled=splitToggle.disabled;
  if(target)focusPeer.textContent=`Focus Shell ${target.epoch}`;
  if(peerState&&peerState.epoch!==target?.epoch){
    peerState=null;clearPaint(peerCanvas);delete peerCanvas.dataset.geometry;
  }
}
function renderSplitBalance(triple=false){
  const share=triple?tripleShares.columns:splitShares[splitAxis];
  const rows=!triple&&splitAxis==='vertical';
  splitBalanceLabel.textContent=rows?'Top pane height':'Left pane width';
  splitBalance.value=String(share);
  splitBalance.setAttribute('aria-valuetext',rows
    ? `${share}% top, ${100-share}% bottom`
    : `${share}% left, ${100-share}% right`);
  panes.style.setProperty('--first-pane-share',`${share}fr`);
  panes.style.setProperty('--second-pane-share',`${100-share}fr`);
  if(triple){
    const top=tripleShares.rows;
    splitStackBalance.value=String(top);
    splitStackBalance.setAttribute('aria-valuetext',`${top}% top right, ${100-top}% bottom right`);
    panes.style.setProperty('--top-stack-share',`${top}fr`);
    panes.style.setProperty('--bottom-stack-share',`${100-top}fr`);
  }
}
splitBalance.addEventListener('input',()=>{
  const triple=tripleIsVisible();
  if(splitBalance.disabled||(!triple&&!observerTarget()))return;
  const share=splitShareValue(splitBalance.value);
  if(share===null)return;
  if(triple)tripleShares.columns=share;
  else splitShares[splitAxis]=share;
  renderSplitBalance(triple);
  // The existing viewport observer fits only the current writable PTY.
});
splitStackBalance.addEventListener('input',()=>{
  if(splitStackBalance.disabled||!tripleIsVisible())return;
  const share=splitShareValue(splitStackBalance.value);
  if(share===null)return;
  tripleShares.rows=share;
  renderSplitBalance(true);
  // Update geometry but do not recreate observers or switch the input owner.
});
splitBalance.addEventListener('change',()=>{
  // A drag may generate many input events. Persist only the committed value.
  const triple=tripleIsVisible();
  if(splitBalance.disabled||(!triple&&!observerTarget()))return;
  const share=splitShareValue(splitBalance.value);
  const current=triple?tripleShares.columns:splitShares[splitAxis];
  if(share!==null&&share===current)
    writePreference(triple?tripleShareKeys.columns:splitShareKey(splitAxis),String(share));
});
splitStackBalance.addEventListener('change',()=>{
  if(splitStackBalance.disabled||!tripleIsVisible())return;
  const share=splitShareValue(splitStackBalance.value);
  if(share!==null&&share===tripleShares.rows)writePreference(tripleShareKeys.rows,String(share));
});
let localTabButtons=[];
function renderLocalTabs(focusActive=false) {
  renderTerminalStage();
  renderSavedLayout();
  renderSplit();
  history.updateSearchControls();
  blocks.updateOwner();
  refreshTerminalControls();
  terminalFocus.refresh();
  tabs.hidden=!invoke||sshSession!==null||!localTabEpochs.size||(!owner&&showLocal.disabled&&!changingTab);
  newTab.disabled=newJshTab.disabled=!owner||changingTab!==null||recovery!==null||restoringLocalLayout||localTabEpochs.size>=MAX_LOCAL_SHELLS;
  retryTab.hidden=!changingTab||changingTab.busy||restoringLocalLayout;
  if(tabs.hidden)return;
  const numbers=[...localTabEpochs].sort((a,b)=>a-b);
  // A refresh with the same roster must not detach a button that is
  // focused or between native pointer-down and click.
  if(localTabButtons.length!==numbers.length||
      numbers.some((number,index)=>localTabButtons[index].dataset.epoch!==String(number))){
    localTabButtons=numbers.map(number=>{
      const tab=document.createElement('button');
      tab.type='button';tab.dataset.epoch=String(number);
      tab.addEventListener('click',()=>{
        if(tabs.hidden||tab.disabled||!localTabButtons.includes(tab))return;
        if(number===epoch)focusTerminal();
        else startLocalTab(number,{focusTerminal:true});
      });
      return tab;
    });
    tabList.replaceChildren(...localTabButtons);
  }
  let activeButton=null;
  const disabled=changingTab!==null||recovery!==null||restoringLocalLayout;
  localTabButtons.forEach((tab,index)=>{
    const number=numbers[index],kind=localShellKinds.get(localTabOwners.get(number)?.session);
    const label=`${kind==='jsh'?'jsh':'Shell'} ${number}`;
    if(tab.textContent!==label)tab.textContent=label;
    if(number===epoch){tab.setAttribute('aria-current','true');activeButton=tab;}
    else tab.removeAttribute('aria-current');
    tab.disabled=disabled;
  });
  if(focusActive)activeButton?.focus();
}
function validatePeerFrame(frame,target){
  const session=localTabOwners.get(target.epoch)?.session??frame?.session;
  if(typeof session!=='string'||!/^[a-f0-9]{32}$/.test(session))throw new Error('invalid inactive shell');
  const next=acceptFull(null,frame,{session,epoch:target.epoch});
  return peerState?.epoch===next.epoch&&peerState.revision>=next.revision?null:next;
}
function validateChannelPeer(preview,target,frameVersion){
  const session=localTabOwners.get(target.epoch)?.session;
  if(typeof session!=='string'||!/^[a-f0-9]{32}$/.test(session))throw new Error('invalid inactive shell');
  const next=acceptChannelPayload(null,preview,{session,epoch:target.epoch,frameVersion},true);
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
function desiredObservers(){
  return tripleIsVisible()?{multi:true,targets:passiveTargets}:{multi:false,targets:[observerTarget(),null]};
}
function sameObservers(left,right){
  return left.multi===right.multi&&left.targets.every((target,index)=>sameObservation(target,right.targets[index]));
}
async function syncObserver(stream){
  if(localStream!==stream||!stream.ready)return;
  if(stream.adjusting)return stream.adjusting;
  stream.adjusting=(async()=>{
    let wanted=desiredObservers();
    while(localStream===stream&&!sameObservers(stream.observers,wanted)){
      const authority={owner:stream.owner,streamId:stream.channel.id};
      if(wanted.multi)await invoke('set_local_observers',{...authority,observers:wanted.targets});
      else await invoke('set_local_observer',{...authority,observer:wanted.targets[0]});
      stream.observers=wanted;wanted=desiredObservers();
    }
  })();
  try{await stream.adjusting;}finally{stream.adjusting=null;}
}
async function toggleTriple(){
  if(tripleToggle.disabled)return;
  const attachment=owner,stream=localStream;
  const known=knownLocalEpochs();
  if(!stream?.ready||!known.includes(epoch)||known.length<3)return;
  const old=tripleEpochs;
  splitLayout=allocateLayoutToken();
  tripleEpochs=old.length?[]:known.slice(0,3);
  if(tripleEpochs.length&&!tripleEpochs.includes(epoch))tripleEpochs[2]=epoch;
  settingSplit=true;renderSplit();
  try{await syncObserver(stream);}
  catch(error){
    if(owner===attachment&&localStream===stream){
      tripleEpochs=old;splitLayout=allocateLayoutToken();renderSplit();
      stream.observers={multi:!desiredObservers().multi,targets:[null,null]};
      try{await syncObserver(stream);}catch{ /* Output authority is unchanged. */ }
      if(owner===attachment)status.textContent=`Three panes unavailable: ${String(error?.message??error)}`;
    }
  }finally{settingSplit=false;renderSplit();recordLocalLayout();}
}
tripleToggle.addEventListener('click',toggleTriple);
passivePanes.forEach(({focus,canvas:surface},index)=>{
  const select=()=>{
    const target=passiveTargets[index];
    if(target&&!focus.disabled)return startLocalTab(target.epoch,{focusTerminal:true});
  };
  focus.addEventListener('click',select);
  surface.addEventListener('click',select);
});
const splitIntent=paneInputIntent();
async function setSplit(axis){
  splitIntent.cancel();
  if(splitToggle.disabled)return;
  if(axis&&localTabEpochs.size<2){
    const configuration=localShellKinds.get(owner.session)==='jsh'?jshLocalTab:plainLocalTab;
    const start=()=>startLocalTab(null,{configuration,focusTerminal:true,splitOnActivate:axis});
    const queue=keyQueue, attachment=owner, focus=document.activeElement, layout=splitLayout;
    // Ctrl+Shift+D can arrive while Enter (or its release) still has a PTY
    // receipt outstanding. Respect the existing no-switch-before-ACK gate.
    if(queue.pending>0&&!queue.locked&&!input.disabled){
      splitIntent.defer(queue,
        ()=>ownsInput(attachment,queue)&&!queue.locked&&!input.disabled&&!splitToggle.disabled&&
          !document.hidden&&!terminalPanel.hidden&&pasteReviewPanel.hidden&&!commandPalette.isOpen()&&
          !ime.isComposing()&&document.activeElement===focus&&splitLayout===layout&&localTabEpochs.size===1,
        ()=>void start(),
        ()=>{status.textContent='Input is still finishing. Try splitting again.';});
      return;
    }
    return start();
  }
  if(axis&&splitEnabled&&splitAxis===axis&&!tripleIsVisible())return;
  tripleEpochs=[];
  if(axis){
    splitAxis=axis;
    writePreference('aether.split-axis',axis);
  }
  if(axis&&!splitEnabled)splitEpochs=localSplitPair([...localTabEpochs].sort((a,b)=>a-b),epoch);
  if(!axis)splitEpochs=[];
  splitEnabled=!!axis;splitLayout=allocateLayoutToken();
  settingSplit=true;renderSplit();
  const attachment=owner;
  try{
    if(localStream)await syncObserver(localStream);
    else if(observerTarget()&&localTransport!=='recovery'){
      clearInterval(timer);timer=setInterval(poll,50);await poll();
    }
  }
  catch(error){if(owner===attachment)status.textContent=`Split pane unavailable: ${String(error?.message??error)}`;}
  finally{settingSplit=false;renderSplit();recordLocalLayout();}
}
splitToggle.addEventListener('click',()=>setSplit(splitEnabled||tripleIsVisible()?null:'horizontal'));
function selectPeer(){
  const target=observerTarget();
  if(target&&!focusPeer.disabled)return startLocalTab(target.epoch,{focusTerminal:true});
}
focusPeer.addEventListener('click',selectPeer);
peerCanvas.addEventListener('click',selectPeer);
function setTabRoster(records){
  // A passive leaf can disappear from the runtime roster independently of
  // Close. Preserve the current writer and collapse only the missing leaf.
  const missing=tripleIsVisible()&&records.includes(epoch)?
    tripleEpochs.filter(number=>!records.includes(number)):[];
  const survivors=missing.length===1?previewCloseSurvivors(localTabOwners.get(missing[0])):null;
  for(const [number,attachment] of localTabOwners){
    if(records.includes(number))continue;
    localTabOwners.delete(number);localDrafts.delete(attachment.session);
    localShellKinds.delete(attachment.session);localPaneIds.delete(attachment.session);localTabGeometry.delete(number);terminalChat.forget(attachment.session);
  }
  localTabEpochs.clear();
  for(const number of records)localTabEpochs.add(number);
  const collapsed=installPreviewSurvivors(survivors);
  if(!collapsed)updateSplitPair();
  renderLocalTabs();
  if(collapsed){
    recordLocalLayout();
    const stream=localStream;
    if(stream)void syncObserver(stream).catch(error=>{
      if(localStream===stream)status.textContent=`Split pane unavailable: ${String(error?.message??error)}`;
    });
  }
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
    const kind=command==='ack_shell'&&typeof args.revision==='string'?'ack_packed':sshCommands[command];
    if(!kind)return Promise.reject(new Error('Unknown SSH terminal command'));
    return invoke('ssh_terminal',{action:{operation:'request',command:{kind,owner:attachment,...args}}});
  }
  if(command==='resize_shell') args.windowPixels=windowPixels(args.rows,args.cols);
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
const keyboardView=()=>localView()??(!recovery&&owner===sshKeyboardOwner?owner:null);
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
    const next=acceptFull(state,frame);paintLiveFrame(next);state=next;describeCursor();renderTerminalStage();
    if(invoke) await invoke('acknowledge',{session:state.session,epoch:state.epoch,revision:state.revision});
    status.textContent=`Rust frame ${state.revision} accepted. No PTY attached.`;
  }catch(e){if(currentGeneration!==generation)return;status.textContent=`Stopped: ${String(e?.message??e)}. Restart the app to reset.`;}
  finally {if(currentGeneration===generation){button.disabled=!invoke&&state!==null;setOpenDisabled(false);attach.disabled=!invoke;}}
});
function validateStampedMouseView(view) {
  if (!view || !Number.isInteger(view.generation) || view.generation < 1 || view.generation > 0xffffffff) {
    throw new Error('invalid terminal mouse state');
  }
  channelCounter(view.layout);
  channelCounter(view.revision);
}
function validateShellStatus(info,expectedEpoch){
  if(!info||info.epoch!==expectedEpoch||typeof info.exited!=='boolean'||typeof info.drained!=='boolean'||(info.write_error!==null&&typeof info.write_error!=='string')||typeof info.dropped_reply_bytes!=='string'||!/^[0-9]{1,20}$/.test(info.dropped_reply_bytes)||typeof info.unsupported!=='string'||!/^[0-9]{1,20}$/.test(info.unsupported))throw new Error('invalid session status');
  const exitNotice=info.exit_output_notice;
  if(exitNotice!=null&&(typeof exitNotice!=='string'||exitNotice.length>256))throw new Error('invalid exit output notice');
  const local = owner?.session !== sshSession;
  if(local){
    validateChatStatus(info.chat);
    acceptLocalPane(info);
    if(info.history_oldest_row!==undefined)channelCounter(info.history_oldest_row);
    if(info.inline_codex_history!==undefined&&typeof info.inline_codex_history!=='boolean')
      throw new Error('invalid inline history state');
  }
  if (local || info.mouse_reporting === true) {
    if (info.mouse !== undefined) {
      validateStampedMouseView(info.mouse);
      if (!['press_only', 'button', 'drag', 'any'].includes(info.mouse.tracking)) {
        throw new Error('invalid terminal mouse tracking');
      }
      if (info.mouse.pixels !== undefined) {
        const {width, height} = info.mouse.pixels ?? {};
        if (![width, height].every(size => Number.isInteger(size) && size > 0 && size <= 0xffffffff)) {
          throw new Error('invalid terminal mouse pixel extent');
        }
      }
    }
    if (info.alternate_scroll !== undefined) validateStampedMouseView(info.alternate_scroll);
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
  if(info.exited||info.write_error){
    const wasWritable=!keyQueue.locked;
    pasteReview.reset();keyQueue.locked=true;input.disabled=send.disabled=resize.disabled=true;
    if(wasWritable)renderTerminalStage();
  }
  if (owner && owner.session === sshSession) {
    if (info.poll_with_status === true && sshPoll?.owner !== owner) {
      sshPoll = sshPollFor(owner,info);
    }
    sshMouseOwner = info.mouse_reporting === true ? owner : null;
    const capable = info.focus_reporting === true ? owner : null;
    if (capable !== sshFocusOwner) {
      sshFocusOwner = capable;
      terminalFocus.refresh();
    }
  }
  if(owner&&owner.session!==sshSession){
    terminalChat.showStatus(info,owner);
    if(info.history_oldest_row!==undefined)history.updateRetained(owner,info.history_oldest_row);
    inlineHistory.update(owner,typeof Channel==='function'&&info.inline_codex_history===true,info.history_oldest_row);
  }
  const mouse = info.mouse ?? (info.alternate_scroll
    ? {...info.alternate_scroll, tracking: 'alternate_scroll'} : null);
  mouseView = canReportMouse(owner) && mouse ? {owner, mouse} : null;
  terminalMouse?.frameChanged();
  workflows.updateOwner();
  if(document.activeElement===imeInput||document.activeElement===canvas)terminalChat.followCursor();
}
function stopTransport(){
  inlineHistory.update(null,false);
  sshPoll = null;
  mouseView = null;
  terminalMouse?.frameChanged();
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
  renderTerminalStage();
  renderSplit();
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
    const peer=target&&event.preview?.layout===target.layout?validateChannelPeer(event.preview,target,stream.frameVersion):null;
    const commitPassive=tripleIsVisible()?passiveSurfaces.prepare(event.preview,stream.frameVersion):null;
    let revision=null;
    if(event.frame!==null){
      const next=acceptChannelPayload(stream.frame,event,{...attachment,frameVersion:stream.frameVersion});
      paintLiveFrame(next);state=next;stream.frame=next;revision=next.revision;describeCursor();
    }
    paintPeer(peer);
    commitPassive?.();
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
async function startTransport(onFirstPoll){
  if(!owner||localTransport==='recovery')return;
  let channel;
  if(owner.session!==sshSession&&localTransport!=='poll'&&typeof Channel==='function'){
    try{channel=new Channel();}catch{/* No subscription was sent. */}
  }
  if(!channel){
    if(tripleEpochs.length&&owner.session!==sshSession){tripleEpochs=[];renderSplit();}
    if(owner.session!==sshSession&&supportsChannelRendition(state?.version)){
      suspendLocalOutput(new Error('local output Channel unavailable'));
      return;
    }
    timer=setInterval(poll,50);return poll(onFirstPoll);
  }
  const stream={owner,channel,frameVersion:CHANNEL_DENSE_VERSION,sequence:0n,frame:null,lastSeen:Date.now(),animation:null,observers:{multi:false,targets:[observerTarget(),null]},ready:false,adjusting:null};
  localStream=stream;
  stream.channel.onmessage=event=>{
    if(localStream!==stream||owner!==stream.owner)return;
    try{return queueLocalStream(stream,decodeLocalChannelEvent(event));}
    catch(error){failLocalStream(stream,error,true);}
  };
  timer=setInterval(()=>{
    if(localStream===stream&&!document.hidden&&Date.now()-stream.lastSeen>10000)
      failLocalStream(stream,new Error('no local output heartbeat'));
  },1000);
  try{await invoke('subscribe_local_shell',{owner:stream.owner,stream:stream.channel,observer:stream.observers.targets[0],
    frameVersion:stream.frameVersion,directFrames:true,binaryRasters:true});}
  catch(error){failLocalStream(stream,error);}
  if(localStream===stream){
    stream.ready=true;renderSplit();
    try{await syncObserver(stream);}
    catch(error){if(localStream===stream)status.textContent=`Split pane unavailable: ${String(error?.message??error)}`;}
  }
}
document.addEventListener?.('visibilitychange',()=>{if(document.hidden)ime.reset();if(localStream)localStream.lastSeen=Date.now();});

async function poll(onAccepted){
  if(epoch===null||changingTab||localStream||localTransport==='recovery'||polling?.epoch===epoch)return;const current=epoch,attachment=owner,ticket={epoch,transport};polling=ticket;
  const batch=sshPoll?.owner===attachment?sshPoll:null;
  const frameVersion=batch?.frameVersion,packed=frameVersion!=null;
  const previousPacked=packed&&state?.version===frameVersion?state:null;
  try{
    const reply=await (batch
      ? invokeAttached(packed?'poll_packed_shell':'poll_ssh_shell',{
        epoch:current,ack_revision:batch.ackRevision,...(packed?{snapshot:previousPacked===null}:{}),
        ...(frameVersion===CHANNEL_DENSE_VERSION?{frame_version:frameVersion}:{}),
      },attachment)
      : invokeAttached('poll_shell',{epoch:current},attachment));
    if(epoch!==current||ticket.transport!==transport)return;
    if(batch&&(reply?.status?.poll_with_status!==true||
        (packed&&reply.status.packed_frame_version!==frameVersion)||
        (reply.update!==null&&typeof reply.update!=='object')))throw new Error('invalid SSH terminal poll reply');
    const update=batch?reply.update:reply;
    if(update!=null){
      const next=packed
        ? acceptChannelUpdate(previousPacked,update,{...attachment,frameVersion},
          previousPacked===null,reply.frame_bytes)
        : acceptUpdate(state,update,attachment);
      paintLiveFrame(next);state=next;describeCursor();
      if(!batch)await invokeAttached('ack_shell',{epoch:current,revision:next.revision},attachment);
    }
    if(epoch!==current||ticket.transport!==transport)return;
    const info=batch?reply.status:await invokeAttached('shell_status',{epoch:current},attachment);
    if(epoch!==current||ticket.transport!==transport)return;
    validateShellStatus(info,current);
    if(batch)batch.ackRevision=update===null?null:state.revision;
    onAccepted?.(info);
    showShellStatus(info,'poll');
    const target=observerTarget();
    if(target){
      try{
        const preview=await invoke('observe_local_shell',{owner:attachment,epoch:target.epoch,revision:peerState?.revision??null});
        if(owner===attachment&&sameObservation(target,observerTarget())&&preview)paintPeer(validatePeerFrame(preview,target));
      }catch(error){if(owner===attachment&&sameObservation(target,observerTarget()))status.textContent=`Split pane unavailable: ${String(error?.message??error)}`;}
    }
    // Finish draining the PTY and acknowledge any last batched update.
    if(info.drained&&!target&&(!batch||batch.ackRevision===null))clearInterval(timer);
    return true;

  }catch(e){
    if(epoch!==current||ticket.transport!==transport)return;
    const isSsh=attachment.session===sshSession;
    sshInputSuspended=isSsh&&!keyQueue.locked&&!keyQueue.pending&&!input.disabled;
    sshReloadNeeded=isSsh&&sshInputSuspended&&ime.isComposing();
    pasteReview.reset();ime.reset();keyQueue.locked=true;clearInterval(timer);
    status.textContent=isSsh?`SSH terminal paused: ${String(e?.message??e)}. Use Recover output snapshot.`:`Shell stopped: ${String(e?.message??e)}. Close/reopen before continuing.`;
    input.disabled=send.disabled=resize.disabled=true;
    renderTerminalStage();
    terminalChat.updateOwner();
  }
  finally{if(polling===ticket)polling=null;}
}
function setActiveControls(writable){
  input.disabled=send.disabled=resize.disabled=!writable||restoringLocalLayout;
  close.disabled=detach.disabled=restoringLocalLayout;
  if (writable && fitSize?.checked) void Promise.resolve().then(requestFit);
}
function localTabInputPending(){
  return !!owner&&(keyQueue.pending>0||(!keyQueue.locked&&input.disabled));
}
function startLocalTab(targetEpoch,{fromRestore=false,focusTerminal=false,configuration=plainLocalTab,splitOnActivate=null}={}) {
  if(sshSession!==null||changingTab||recovery||restoringLocalLayout&&!fromRestore||
      (!owner&&(showLocal.disabled||targetEpoch===null)))return;
  if(!fromRestore&&localTabInputPending()){
    status.textContent='Wait for the current input request to finish before switching shells.';
    return;
  }
  const preserveInput=owner!==null&&!keyQueue.locked&&(fromRestore||!input.disabled);
  splitIntent.cancel();
  changingTab={source:owner,targetEpoch,preserveInput,resizeDisabled:resize.disabled,hadChannel:localStream!==null||localTransport==='recovery',focusTerminal,configuration,
    previousFrame:state,busy:false,view:null,splitOnActivate};
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
        preserveInput:change.preserveInput,...options,...launch,
        ...(change.targetEpoch===null?{windowPixels:initialWindowPixels()}:{})});
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
    changingTab=null;reportPaintedWindowPixels(state);recordLocalLayout();renderLocalTabs(!change.focusTerminal);
    if(change.focusTerminal)focusTerminal();
    await startTransport();
    if(owner===next.owner&&change.splitOnActivate)await setSplit(change.splitOnActivate);
  }catch(e){
    if(changingTab!==change)return;
    clearTerminalPaint();
    status.textContent=`Shell tab activation unconfirmed: ${String(e?.message??e)}. Retry the switch to recover the view.`;
    change.busy=false;renderLocalTabs();retryTab.focus();
  }
}
newTab.addEventListener('click',()=>{if(!newTab.disabled)return startLocalTab(null,{focusTerminal:true});});
const jshLocalTab={shell:'jsh',presentation:'terminal'};
newJshTab.addEventListener('click',()=>{if(!newJshTab.disabled)return startLocalTab(null,{configuration:jshLocalTab,focusTerminal:true});});
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
  protectSavedLayout=true;
  splitEnabled=!!layout.split;splitLayout=allocateLayoutToken();
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
    splitEpochs=layout.split?(layout.panes??[0,1]).map(index=>created[index]):[];splitLayout=allocateLayoutToken();
    if(owner.epoch!==created[layout.active]){
      await startLocalTab(created[layout.active],{fromRestore:true});
      if(changingTab)return;
      if(owner.epoch!==created[layout.active]||keyQueue.locked)throw new Error(status.textContent);
    }
    if(layout.triple){
      if(!localStream?.ready||localTransport!=='channel')throw new Error('Three panes require the local output stream.');
      tripleEpochs=layout.triple.map(index=>created[index]);
    }
    renderSplit();
    if(localStream)await syncObserver(localStream);
    if(layout.triple&&(!tripleIsVisible()||!localStream?.ready))throw new Error('Three pane output was not confirmed.');
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
    if(complete){protectSavedLayout=false;recordLocalLayout();}
    renderLocalTabs();
    if(complete)focusTerminal();
  }
});

async function openShell({destination=null,retained=null,fromRestore=false,configuration=plainLocalTab}={}){
  if(epoch!==null||sshSession||restoringLocalLayout&&!fromRestore)return;
  const attempt=++generation;transport++;sshInputSuspended=false;
  filePreview.dismiss();pasteReview.reset();history.reset();setOpenDisabled(true);attach.disabled=button.disabled=true;
  status.textContent=destination!==null?`Opening SSH terminal on ${destination}…`:'Opening local shell…';
  const integrated=configuration.shell==='jsh';
  try{
    const options=channelSnapshots();
    const launch=configuration.paneId?{integrated,paneHistoryId:configuration.paneId}:{integrated};
    const result=destination!==null?await invoke('ssh_terminal',{action:{operation:'open',destination}})
      :retained?await invoke('open_second_local_shell',{epoch:retained,...options,windowPixels:initialWindowPixels()})
      :await invoke('open_shell',{...launch,...options,windowPixels:initialWindowPixels()});
    if(generation!==attempt)return;
    const paneId=destination===null?acceptLocalPane(result):undefined;
    const opened=destination===null&&paneId!==undefined?acceptOwner({
      session:result.session,epoch:result.epoch,generation:result.generation,
    }):acceptOwner(destination===null?result:result?.owner??result);
    if(retained&&(opened.epoch<=retained||opened.session===detachedLocal?.session||opened.generation!==1))
      throw new Error('Second local shell did not match the open response.');
    owner=opened;sshSession=destination===null?null:opened.session;
    sshKeyboardOwner=destination!==null&&result?.keyboard_event_reporting===true?opened:null;
    sshPoll=destination===null?null:sshPollFor(opened,result);
    localTransport='channel';localInputSuspended=false;
    if(destination===null){detachedLocal=null;restoreLocalDraft(opened);rememberLocalTab(opened,configuration,paneId);}
    const previousEpoch=epoch;epoch=owner.epoch;updateSplitPair(previousEpoch);keyQueue={chain:Promise.resolve(),pending:0,locked:false};state=null;
    setActiveControls(true);
    if(destination===null){setLocalTools(true);recordLocalLayout();}
    renderLocalTabs();
    status.textContent=retained?'Second local shell opened; the first remains retained.':destination!==null?`SSH shell connected to ${destination}.`:integrated?'jsh with command blocks':'Local /bin/sh';
    if(destination!==null&&typeof result?.save_warning==='string')status.textContent+=' '+result.save_warning;
    if(!fromRestore)focusTerminal();
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
    const next=remote?acceptSshSnapshot(result,nextOwner):acceptAttachedSnapshot(result.frame,nextOwner,options);
    if(saved)sshSession=nextOwner.session;
    if(!remote){detachedLocal=null;restoreLocalDraft(nextOwner);rememberLocalTab(nextOwner);}
    const previousEpoch=epoch;owner=nextOwner;epoch=owner.epoch;updateSplitPair(previousEpoch);keyQueue={chain:Promise.resolve(),pending:0,locked:true};
    localTransport='channel';localInputSuspended=false;
    input.disabled=send.disabled=resize.disabled=true;close.disabled=detach.disabled=false;
    paintLiveFrame(next);state=next;describeCursor();
    const confirmation=await confirmSnapshot(nextOwner,next);
    if(owner!==nextOwner||!confirmation)return;
    status.textContent=saved?'Reconnected to the saved SSH shell on '+saved.destination+' in view-only mode.'
      :sshSession===null?'Reconnected to the existing shell in view-only mode.':'Reconnected to the SSH shell in view-only mode.';
    if(sshSession===null){
      setLocalTools(true);
      recordLocalLayout();
      renderLocalTabs();
      if(selected!==null)showLocal.focus();
    }else renderTerminalStage();
    if(selected===null)focusTerminal();
    if(!confirmation.polling)await startTransport();
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
  renderTerminalStage();
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
// The opt-in L layout is A | (B above C). Collapse the closed leaf's
// immediate split only; the parked roster never participates in the choice.
function previewCloseSurvivors(attachment){
  if(!tripleIsVisible()||!tripleEpochs.includes(attachment.epoch))return null;
  const [left,top,bottom]=tripleEpochs;
  const closed=attachment.epoch;
  const peers=tripleEpochs.filter(value=>value!==closed).map(value=>localTabOwners.get(value));
  if(peers.length!==2||peers.some(peer=>!peer))return null;
  return {axis:closed===left?'vertical':'horizontal',peers,target:closed===top?bottom:top};
}
function installPreviewSurvivors(survivors){
  if(!survivors||!survivors.peers.every(peer=>localTabEpochs.has(peer.epoch)&&
      localTabOwners.get(peer.epoch)?.session===peer.session))return false;
  tripleEpochs=[];
  splitEnabled=true;
  splitEpochs=survivors.peers.map(peer=>peer.epoch);
  splitAxis=survivors.axis;
  writePreference('aether.split-axis',splitAxis);
  splitLayout=allocateLayoutToken();
  return true;
}
close.addEventListener('click',async()=>{
  // SSH and local shells can have the same numeric epoch. Retire the poll
  // before Close can give up that view, even if the close response is lost.
  if(changingTab||!owner||close.disabled)return;
  const old=epoch,oldOwner=owner,survivors=previewCloseSurvivors(oldOwner);
  generation++;transport++;polling=null;setLocalTools(false);filePreview.dismiss();pasteReview.reset();
  ime.reset();history.reset();stopTransport();epoch=null;owner=null;renderLocalTabs();
  resize.disabled=close.disabled=detach.disabled=input.disabled=send.disabled=true;
  let result;
  try{result=await invokeAttached('close_shell',{epoch:old},oldOwner);}
  catch(e){
    epoch=old;owner=oldOwner;blocks.updateOwner();terminalChat.updateOwner();close.disabled=detach.disabled=false;
    status.textContent=`Close error: ${String(e?.message??e)}. Retry Close to finish cleanup.`;
    return;
  }
  let resume=null;
  if(oldOwner.session!==sshSession){
    input.value='';localDrafts.delete(oldOwner.session);localTabEpochs.delete(oldOwner.epoch);
    localTabOwners.delete(oldOwner.epoch);localShellKinds.delete(oldOwner.session);localPaneIds.delete(oldOwner.session);
    localTabGeometry.delete(oldOwner.epoch);terminalChat.forget(oldOwner.session);
    if(installPreviewSurvivors(survivors)){
      resume=survivors.target;
    }else if(localTabEpochs.size<2){
      splitEnabled=false;splitEpochs=[];tripleEpochs=[];
    }else updateSplitPair();
    if(resume===null)recordLocalLayout();
  }
  const warning=typeof result?.save_warning==='string'?result.save_warning:'';
  status.textContent=result?.cleanup_pending===false?'Terminal closed; direct child reaped. Descendant containment is not provided.':
    'Terminal closed; direct-child cleanup handed to bounded supervisor and may still be pending. Descendant containment is not provided.';
  if(warning)status.textContent+=' '+warning;
  state=null;clearTerminalPaint();sshSession=null;sshInputSuspended=false;sshKeyboardOwner=null;
  setOpenDisabled(false);attach.disabled=button.disabled=false;renderLocalTabs();
  if(resume!==null){
    await startLocalTab(resume,{focusTerminal:true});
    if(epoch===resume&&warning)status.textContent+=' '+warning;
  }
});
document.querySelector('#command').addEventListener('submit',async event=>{
  event.preventDefault();if(epoch===null||input.disabled)return;
  // Reserve one byte for the appended newline before creating an encoded copy.
  if(input.value.length>4095){status.textContent='Input exceeds 4096-byte bound';return;}
  // jsh's interactive line editor consumes the physical Enter byte.
  const enter=localShellKinds.get(owner?.session)==='jsh'?'\r':'\n';
  const bytes=new TextEncoder().encode(input.value+enter);if(bytes.length>4096){status.textContent='Input exceeds 4096-byte bound';return;}
  splitIntent.cancel();
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
 finally{if(ownsInput(attachment,queue)){
   if(!failed&&!queue.locked){send.disabled=false;input.disabled=false;if(!commandPalette.isOpen())input.focus();terminalFocus.refresh();}
   commandPalette.refreshAvailability();
 }}
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
  colsInput.value=String(size.cols);rowsInput.value=String(size.rows);
  // The painted frame can lag an in-flight resize. A restored viewport
  // still needs to replace the pending intent even if the old paint matches.
  if (!sizes.busy() && state?.rows.length === size.rows && state.cols === size.cols) return;
  void sizes.request(size.rows, size.cols);
}
function setFit(enabled){
  fitSize.checked=enabled;
  writePreference('aether.fit-terminal',enabled?'on':'off');
  lastFit=null;
  fitViewport.classList.toggle('fit-terminal',enabled);
  if(enabled)requestFit();
  else sizes.cancelPending();
}
if (fitSize && fitViewport) {
  fitSize.disabled = !invoke;
  fitSize.checked = readPreference('aether.fit-terminal') !== 'off';
  fitViewport.classList.toggle('fit-terminal', fitSize.checked);
  fitSize.addEventListener('change',()=>setFit(fitSize.checked));
  if (invoke && typeof ResizeObserver === 'function') new ResizeObserver(requestFit).observe(fitViewport);
}
function refreshTerminalScale() {
  const frame = visibleLiveFrame();
  if (frame && lastPaintScale !== (globalThis.devicePixelRatio || 1)) paintTerminalFrame(frame);
}
function watchTerminalScale() {
  const match = globalThis.matchMedia?.(`(resolution: ${globalThis.devicePixelRatio || 1}dppx)`);
  match?.addEventListener?.('change', () => {
    refreshTerminalScale();
    watchTerminalScale();
  }, {once:true});
}
if (invoke) {
  globalThis.addEventListener?.('resize', refreshTerminalScale);
  watchTerminalScale();
}
resize.addEventListener('click', async () => {
  if (epoch === null || input.disabled) return;
  try {
    const {rows, cols} = dimensions(Number(rowsInput.value), Number(colsInput.value));
    if(fitSize?.checked)setFit(false);
    await sizes.request(rows, cols);
  } catch (e) {
    status.textContent = String(e?.message ?? e);
  }
});
function describeCursor(frame=visibleLiveFrame()){
  refreshHyperlinkHint(frame);
  if(!frame)return;
  const surface=selectionOverlay?.parentElement ?? canvas;
  const left=`${(surface.offsetLeft||0)+frame.cursor_col*12}px`;
  const top=`${(surface.offsetTop||0)+frame.cursor_row*24}px`;
  for(const layer of [imeInput,terminalPreedit]){
    layer.style.left=left;layer.style.top=top;
  }
  rememberLocalGeometry(state);
  const size=`${frame.cols} × ${frame.rows.length}`;
  if(terminalSize.textContent!==size)terminalSize.textContent=size;
  document.querySelector('#cursor-status').textContent=`${frame.cols} × ${frame.rows.length} cells · cursor row ${frame.cursor_row+1}, column ${frame.cursor_col+1} · ${frame.cursor_shape}${frame.cursor_visible?'':' (hidden)'}${frame.pending_wrap?' · wrap pending':''}`;
}
function hyperlinkAt(event,frame=visibleLiveFrame()){
  if(!owner||owner.session===sshSession||!supportsChannelRendition(frame?.version)||
      frame.session!==owner.session||frame.epoch!==owner.epoch)return null;
  const point=terminalCellAt(frame,canvas,event.clientX,event.clientY);
  if(!point)return null;
  const cells=frame.rows[point.row];
  const cell=cells[point.col].width===0?cells[point.col-1]:cells[point.col];
  const target=frame.hyperlinks[cell.hyperlink_id];
  return target?{...point,target,revision:frame.revision}:null;
}
function refreshHyperlinkHint(frame=visibleLiveFrame()){
  const pointer=hoveredLinkPoint;
  if(!owner||owner!==pointer?.attachment||!frame)hoveredLinkPoint=null;
  const link=hoveredLinkPoint?hyperlinkAt(pointer,frame):null;
  const visible=link&&(!document.elementFromPoint||document.elementFromPoint(pointer.clientX,pointer.clientY)===canvas);
  canvas.title=visible?`Ctrl-click to open ${link.target}`:'';
  canvas.style.cursor=visible&&pointer.ctrlKey?'pointer':'';
}
canvas.addEventListener('pointermove',event=>{
  hoveredLinkPoint={attachment:owner,clientX:event.clientX,clientY:event.clientY,ctrlKey:event.ctrlKey};
  refreshHyperlinkHint();
});
canvas.addEventListener('pointerleave',()=>{hoveredLinkPoint=null;refreshHyperlinkHint();});
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
  inlineHistory.reset();
  const frame=visibleLiveFrame();
  if(!frame)return;
  const viewport=document.querySelector('#active-pane .viewport');
  viewport.scrollTo(cursorTarget(frame,selectionOverlay?.parentElement ?? canvas,viewport));
}
document.querySelector('#reveal-cursor').addEventListener('click',()=>{revealLiveCursor();focusTerminal();});


// Browser supplies logical events only; Rust chooses all terminal wire sequences.
let keyQueue={chain:Promise.resolve(),pending:0,locked:false};
const terminalFocus = installTerminalFocus({
  document, window: globalThis,
  getOwner: focusOwner,
  isTerminalElement: element => element === canvas || element === imeInput,
  send: enqueueFocus,
});
function focusOwner() {
  return owner && (owner.session !== sshSession || owner === sshFocusOwner) ? owner : null;
}
function canReportMouse(attachment) {
  return attachment !== null && (attachment.session !== sshSession || attachment === sshMouseOwner);
}
function ownsInput(attachment,queue){
  return changingTab===null&&attachment!==null&&owner===attachment&&keyQueue===queue;
}
function enqueueFocus(attachment, focused) {
  const queue = keyQueue;
  const advisory = queue.focusPending ?? 0;
  if (!ownsInput(attachment, queue) || input.disabled || queue.locked || recovery ||
      advisory >= 2 || queue.pending >= 126) return false;
  queue.focusPending = advisory + 1;
  queue.chain = queue.chain.then(async () => {
    if (!ownsInput(attachment, queue) || attachment !== focusOwner() ||
        input.disabled || queue.locked || recovery) return;
    try {
      await invokeAttached('focus_shell', {epoch: attachment.epoch, focused}, attachment);
    } catch {
      // View retirement or a pending write can race this advisory input.
    }
  }).finally(() => {
    queue.focusPending--;
    terminalFocus.refresh();
  });
  return true;
}
function enqueueLogical(command,payload,onReceipt,permit){
  if(epoch===null||input.disabled)return;
  const queue=keyQueue;
  const mouse=command==='mouse_shell';
  const advisoryKey=command==='key_event_shell' &&
    (payload.event.phase==='release'||modifierKey(payload.event.name));
  if(!advisoryKey)splitIntent.cancel();
  if(!mouse&&!advisoryKey)inlineHistory.reset();
  if (mouse && (!canReportMouse(owner) || queue.pending >= (payload.report.kind === 'release' ? 128 : 126))) return;
  if(queue.pending>=128){pasteReview.reset();queue.locked=true;status.textContent='Keyboard queue full; input locked. Close/reopen before retrying.';input.disabled=send.disabled=true;return;}
  const current=epoch,attachment=owner;queue.pending++;
  if(queue.pending===1)commandPalette.refreshAvailability();
  queue.chain=queue.chain.then(async()=>{
    if(!ownsInput(attachment,queue)||queue.locked||mouse&&!canReportMouse(attachment)||permit&&!permit())return;
    try{
      const result=await invokeAttached(command,{epoch:current,...payload},attachment);
      if(ownsInput(attachment,queue))onReceipt?.(result);
    }
    catch(e){if(!mouse&&ownsInput(attachment,queue)){pasteReview.reset();queue.locked=true;input.disabled=send.disabled=true;renderTerminalStage();status.textContent=`Input not accepted: ${String(e?.message??e)}. Close/reopen before retrying.`;}}
  }).finally(()=>{
    queue.pending--;
    if(queue.pending===0&&ownsInput(attachment,queue))commandPalette.refreshAvailability();
    splitIntent.settle(queue);
    if (queue.pending === 125) terminalFocus.refresh();
  });
  return queue.chain;
}
function keydown(event){
  terminalKeys.clearText();
  // Respect an earlier control or capture handler that already owns this key.
  if(event.defaultPrevented||(sshSession!==null&&recovery))return;
  if(epoch===null||input.disabled||ime.isComposing()||event.isComposing||event.keyCode===229||event.getModifierState?.('AltGraph')||event.key==='Dead'||event.key==='Process')return;
  if(pasteReview.dismissFromOrigin(event))return;
  if(workflows.consumeEnterRepeat(event)){event.preventDefault();return;}
  if(inlineHistory.page(event)){event.preventDefault();return;}
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
  if(keyboardView()){
    if(terminalKeys.down(event,event.currentTarget===imeInput))event.preventDefault();
    return;
  }
  if(event.metaKey)return;
  const named=/^(Arrow(Up|Down|Left|Right)|Home|End|Insert|Delete|PageUp|PageDown|F([1-9]|1[0-2])|Enter|Escape|Backspace|Tab)$/;
  const keypad=/^Numpad([0-9]|Decimal|Divide|Multiply|Subtract|Add|Enter|Equal|Comma)$/.test(event.code)
    ? {code:event.code,num_locked:event.getModifierState?.('NumLock')===true}
    : null;
  const namedKey=named.test(event.key)||keypad?.code==='Numpad5'&&event.key==='Clear';
  if(!namedKey&&[...event.key].length!==1)return;
  if(event.currentTarget===imeInput&&!namedKey&&!keypad&&!event.ctrlKey&&!event.altKey)return;
  event.preventDefault();enqueueLogical('key_shell',{name:event.key,shift:event.shiftKey,alt:event.altKey,ctrl:event.ctrlKey,...(keypad?{keypad}:{})});
}
canvas.addEventListener('keydown',keydown);imeInput.addEventListener('keydown',keydown);
canvas.addEventListener('keyup',terminalKeys.up);imeInput.addEventListener('keyup',terminalKeys.up);
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
  document.querySelector('#terminal-history').open=true;
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
    return frame.session!==sshSession&&row.wrapped
      ? text : text.replace(/ +$/,'')+(index+1<frame.rows.length?'\n':'');
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
    const next=isLocal?acceptAttachedSnapshot(result.frame,nextOwner,options):acceptSshSnapshot(result,nextOwner);
    if(keyQueue.pending||(!keyQueue.locked&&input.disabled&&!restoringLocalLayout)){
      pasteReview.reset();keyQueue.locked=true;input.disabled=send.disabled=true;
    }
    const resumeInput=!isLocal&&result.input_allowed===true&&!keyQueue.pending&&
      (sshInputSuspended||(!input.disabled&&!keyQueue.locked));
    owner=nextOwner;
    if(isLocal)rememberLocalTab(nextOwner);
    else sshKeyboardOwner=result.input_allowed===true&&result.keyboard_event_reporting===true?nextOwner:null;
    paintLiveFrame(next);state=next;describeCursor();
    const confirmation=await confirmSnapshot(nextOwner,next,resumeInput);
    if(epoch!==current||transport!==ticket||owner!==nextOwner||!confirmation)return;
    if(!isLocal){
      if(!confirmation.polling){
        if(result.input_allowed!==true){pasteReview.reset();keyQueue.locked=true;input.disabled=send.disabled=resize.disabled=true;}
        else if(sshInputSuspended&&!keyQueue.pending){keyQueue.locked=false;input.disabled=send.disabled=resize.disabled=false;}
      }
      sshInputSuspended=false;
      status.textContent=input.disabled?'SSH output snapshot recovered. Input remains disabled.':'SSH output snapshot recovered.';
    }else if(localTransport==='recovery'){
      localTransport=options.frameVersion?'channel':'poll';
      localFrameRecovery=false;
      if(localInputSuspended&&result.input_allowed===true&&!keyQueue.pending){
        keyQueue.locked=false;input.disabled=send.disabled=resize.disabled=restoringLocalLayout;
      }
      localInputSuspended=false;status.textContent=localTransport==='poll'
        ?'Local output recovered with compatibility polling.':'Local output snapshot recovered.';
    }else{status.textContent=previousStatus??'Output snapshot recovered. Existing input safety locks remain unchanged.';}
    if(!confirmation.polling)await startTransport();
  }catch(e){if(epoch===current&&transport===ticket)status.textContent=`Recovery failed: ${String(e?.message??e)}. Retry recovery or close.`;}
  finally{if(recovery===operation){recovery=null;if(epoch===current&&transport===ticket){detach.disabled=false;setLocalTools(true);renderLocalTabs();}}}
}
recoverButton.addEventListener('click',()=>recoverOutput());


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
    const next=acceptSshSnapshot(result,nextOwner);
    restoreSshReloadView(view);
    owner=nextOwner;epoch=nextOwner.epoch;sshSession=nextOwner.session;state=next;
    sshKeyboardOwner=grant&&result.keyboard_event_reporting===true?nextOwner:null;
    keyQueue={chain:Promise.resolve(),pending:0,locked:true};input.disabled=send.disabled=resize.disabled=true;
    close.disabled=detach.disabled=false;
    paintLiveFrame(next);describeCursor();renderLocalTabs();
    const confirmation=await confirmSnapshot(nextOwner,next,grant);
    if(owner!==nextOwner||!confirmation)return;
    if(!confirmation.polling&&grant){keyQueue.locked=false;input.disabled=send.disabled=resize.disabled=false;}
    status.textContent=input.disabled?'SSH view recovered. Input remains disabled.':'SSH output recovered in a fresh input view.';
    if(!confirmation.polling)await startTransport();
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

const historyPanel=document.querySelector('#terminal-history');
const historyDock=document.querySelector('#terminal-history-dock');
const sessionTools=document.querySelector('#session-tools');
const workspaceTools=document.querySelector('#terminal-tools');
const historyQuery=document.querySelector('#history-find-query');
let historyOrigin,inPlaceHistory=false;
function dockHistory(inPlace) {
  if(inPlaceHistory===inPlace)return;
  inPlaceHistory=inPlace;
  historyPanel.classList.toggle('history-in-place',inPlace);
  (inPlace?activePane:historyDock).append(historyPanel);
}
document.querySelector('#terminal-history > summary').addEventListener('click',()=>{
  if(!historyPanel.open)historyOrigin=owner;
});
function closeHistory(restoreFocus=true) {
  const origin=historyOrigin;
  historyOrigin=undefined;
  history.dismiss();
  historyPanel.open=false;
  const wasInPlace=inPlaceHistory;
  dockHistory(false);
  if(restoreFocus&&origin!==undefined&&owner===origin&&!changingTab&&!recovery){
    if(wasInPlace)(input.disabled?canvas:imeInput).focus({preventScroll:true});
    else focusTerminal();
  }
}
historyPanel.addEventListener('keydown',event=>{
  if(!historyPanel.open||event.key!=='Escape'||event.defaultPrevented||
      event.isComposing||event.keyCode===229||ime.isComposing())return;
  event.preventDefault();
  closeHistory();
});
function revealPanel(panel,focus=panel.querySelector('summary')){
  panel.open=true;
  panel.scrollIntoView({block:'start'});
  focus?.focus({preventScroll:true});
}
function showHistory(search=false){
  if(!historyPanel.open||historyOrigin===undefined)historyOrigin=owner;
  const canFind=!document.querySelector('#history-find-form').hidden;
  dockHistory(search&&canFind);
  if(inPlaceHistory){
    historyPanel.open=true;
    historyQuery.focus({preventScroll:true});
  }else revealPanel(historyPanel);
  if((!search||!canFind)&&owner&&!document.querySelector('#history-output').value)
    document.querySelector('#history-refresh').click();
}
function showTool(id){
  const tool=document.querySelector(id);
  workspaceTools.open=true;
  revealPanel(tool);
}
for(const [button,panel,show] of [
  ['#show-history',historyPanel,()=>showHistory()],
  ['#show-session-tools',sessionTools,()=>revealPanel(sessionTools)],
  ['#show-workspace-tools',workspaceTools,()=>revealPanel(workspaceTools)],
]){
  const control=document.querySelector(button);
  control.setAttribute('aria-expanded','false');
  panel.addEventListener('toggle',()=>{
    control.setAttribute('aria-expanded',String(panel.open));
    if(panel===historyPanel&&!panel.open){historyOrigin=undefined;history.dismiss();dockHistory(false);}
  });
  control.addEventListener('click',()=>{
    if(panel.open){
      if(panel===historyPanel){if(inPlaceHistory)showHistory();else closeHistory();}
      else {panel.open=false;focusTerminal();}
    } else show();
  });
}
function showSsh(){revealPanel(sessionTools,sshHost);}
sshConnect.addEventListener('click',showSsh);
document.querySelector('#history-back-live').addEventListener('click',()=>closeHistory());
for(const control of document.querySelectorAll('.return-to-terminal')){
  control.addEventListener('click',()=>{
    control.closest('details').open=false;
    focusTerminal();
  });
}

function cycleTabs(step){
  const ordered=[...localTabEpochs].sort((a,b)=>a-b);
  const current=ordered.indexOf(epoch);
  const selected=current<0?(step>0?0:ordered.length-1):(ordered.length+current+step)%ordered.length;
  if(ordered[selected]!==epoch)startLocalTab(ordered[selected],{focusTerminal:true});
}
const canCycleTabs=()=>!tabs.hidden&&!changingTab&&!recovery&&!restoringLocalLayout&&
  localTabEpochs.size>(owner?1:0);
const canUseHistory=()=>!!owner&&!changingTab&&!recovery&&!document.querySelector('#history-find-form').hidden;
function clickAction(label,id,hint,keys,unavailableReason){
  const target=document.querySelector(id);
  return {label,hint,keys,unavailableReason,enabled:()=>!target.hidden&&!target.disabled,run:()=>target.click()};
}
async function copyTerminalSelection(){
  const attachment=owner, selection=liveSelection?.snapshot();
  if(!attachment||!selection?.text)return;
  try{
    await invoke('copy_terminal_text',{text:selection.text});
    if(owner===attachment)status.textContent='Terminal selection copied.';
  }catch(error){
    if(owner===attachment)status.textContent=String(error?.message??error);
  }
}
// Resolve the visible neighbor, never the next tab in the parked roster.
function directionalPane(direction){
  if(splitToggle.disabled||maximizedPane)return null;
  const three=tripleIsVisible();
  if(!three&&!observerTarget())return null;
  const tree=retainedPaneTree(three?tripleEpochs:splitEpochs,splitAxis,three);
  if(tree&&three){
    tree.share=tripleShares.columns*10;
    tree.second.share=tripleShares.rows*10;
  }
  return directionalPaneNeighbor(tree,epoch,direction);
}
const directionalPaneActions=['left','right','up','down'].map(direction=>({
  label:`Focus pane ${direction}`,
  enabled:()=>directionalPane(direction)!==null&&!localTabInputPending(),
  unavailableReason:()=>directionalPane(direction)!==null&&localTabInputPending()
    ? 'Wait for the current terminal input to finish.' : 'No visible local pane in this direction.',
  run:()=>{
    const target=directionalPane(direction);
    if(target!==null)return startLocalTab(target,{focusTerminal:true});
  },
}));
const actions=[
  {label:'New local shell',hint:'Ctrl+Shift+T',keys:['Shift+t'],
    enabled:()=>owner?!newTab.disabled:!open.disabled,run:()=>(owner?newTab:open).click()},
  {label:'New jsh shell',enabled:()=>owner?!newJshTab.disabled:!openIntegrated.disabled,
    run:()=>(owner?newJshTab:openIntegrated).click()},
  {label:'Connect over SSH',enabled:()=>!sshConnect.disabled,run:showSsh},
  clickAction('Restore shell tabs','#restore-local-layout'),
  clickAction('Reconnect view','#attach'),
  clickAction('Close shell','#close','Ctrl+Shift+W',['Shift+w']),
  {label:'Next tab',hint:'Ctrl+Tab',keys:['tab','pagedown'],enabled:canCycleTabs,unavailableReason:()=> 'Requires at least two local shell tabs.',run:()=>cycleTabs(1)},
  {label:'Previous tab',hint:'Ctrl+Shift+Tab',keys:['Shift+tab','pageup'],enabled:canCycleTabs,unavailableReason:()=> 'Requires at least two local shell tabs.',run:()=>cycleTabs(-1)},
  {label:'Split left and right',hint:'Ctrl+Shift+E',keys:['Shift+e'],enabled:()=>!splitToggle.disabled,unavailableReason:()=> 'Requires two available local shell tabs.',run:()=>setSplit('horizontal')},
  {label:'Split top and bottom',hint:'Ctrl+Shift+D',keys:['Shift+d'],enabled:()=>!splitToggle.disabled,unavailableReason:()=> 'Requires two available local shell tabs.',run:()=>setSplit('vertical')},
  clickAction('Toggle three panes','#split-three-local'),
  {label:'Maximize active pane',enabled:()=>!maximizedPane&&!maximizePane.hidden&&!maximizePane.disabled,
    unavailableReason:()=> 'Requires a visible split layout.',run:togglePaneMaximize},
  {label:'Restore split layout',enabled:()=>!!maximizedPane&&!maximizePane.disabled,run:togglePaneMaximize},
  ...directionalPaneActions,
  {label:'Return to one pane',enabled:()=>(splitEnabled||tripleIsVisible())&&!splitToggle.disabled,run:()=>setSplit(null)},
  {label:'Search terminal history',hint:'Ctrl+Shift+F',keys:['Shift+f'],enabled:canUseHistory,unavailableReason:()=> 'Requires a local terminal with searchable history.',run:()=>showHistory(true)},
  {label:'Copy terminal selection',hint:'Ctrl+Shift+C',keys:['Shift+c'],
    enabled:()=>!!owner&&!copySelection.disabled,unavailableReason:()=> 'Select text in the attached terminal first.',run:copyTerminalSelection},
  clickAction('Detach terminal view','#detach'),
  clickAction('Recover output snapshot','#recover'),
  clickAction('Terminal Chat','#terminal-chat-toggle'),
  clickAction('Workflows','#workflows',undefined,undefined,()=> 'Requires an active writable jsh prompt.'),
  {label:'Session controls',run:()=>revealPanel(sessionTools)},
  {label:'Workspace tools',run:()=>revealPanel(workspaceTools)},
];
for(const [label,id] of [
  ['Agent Workspace','#agent-workspace'],['SSH Files','#ssh-files'],['Remote Workspace','#ssh-workspace'],
  ['Vim9 Workbench','#vim9-workbench'],['Notebook','#local-notebook'],['Local file preview','#local-file-tools'],
]){
  actions.push({label,enabled:()=>!document.querySelector(id).hidden,run:()=>showTool(id)});
}
if(invoke)actions.push(clickAction('Remote Desktop','#remote-tab'));
const availableActions=actions.map(action=>({...action,
  enabled:()=>!terminalPanel.hidden&&(!action.enabled||action.enabled()),
  unavailableReason:()=>terminalPanel.hidden?'Switch to Terminal first.':action.unavailableReason?.(),
}));
if(invoke)availableActions.push({
  label:'Terminal',enabled:()=>terminalPanel.hidden,run:()=>document.querySelector('#terminal-tab').click(),
});
const commandPalette=installTerminalShell(document,{
  actions:availableActions,
  isComposing:()=>ime.isComposing(),
  paletteScope:target=>pasteReviewPanel.hidden&&(target===document.body||
    !!target?.closest?.('#app-header, #terminal-main, #remote-panel')),
  keyboardScope:target=>!terminalPanel.hidden&&pasteReviewPanel.hidden&&
    (target===document.body||!!target?.closest?.('#terminal-stage, #app-header, #terminal-history')),
});
renderTerminalStage();
void claimSshReload();
