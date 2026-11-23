import {acceptFull,acceptUpdate,clearPaint,paint} from './frames.js';
import {installIme} from './ime.js';

const uint=value=>Number.isInteger(value)&&value>0&&value<=0xffffffff;
const conversationId=value=>typeof value==='string'&&value.startsWith('jterm:cli:v1:')&&value.length<=32768;
const boundedText=(value,limit)=>typeof value==='string'&&value.length<=limit;
const CATALOG_PAGE_MEMORY=4;
const keys=/^(Arrow(Up|Down|Left|Right)|Home|End|Insert|Delete|PageUp|PageDown|F([1-9]|1[0-2])|Enter|Escape|Backspace|Tab)$/;

export function validateWorkspacePage(page){
  if(!uint(page?.generation)||!Array.isArray(page.rows)||page.rows.length>32||
      !(page.next_before===null||typeof page.next_before==='string'&&/^[0-9]{1,20}$/.test(page.next_before))){
    throw new Error('Invalid Workspace catalog receipt.');
  }
  const ids=new Set();
  for(const row of page.rows){
    if(!conversationId(row?.id)||ids.has(row.id)||!boundedText(row.title,8192)||
       !boundedText(row.provider,128)||!boundedText(row.directory,8192)||
       !boundedText(row.status,128)||typeof row.reopenable!=='boolean'){
      throw new Error('Invalid Workspace conversation.');
    }
    ids.add(row.id);
  }
  return page;
}

export function validateWorkspaceAttachment(receipt,conversation){
  const identity=receipt?.identity,owner=identity?.owner;
  if(identity?.conversation!==conversation||typeof owner?.session!=='string'||
     !/^[0-9a-f]{32}$/.test(owner.session)||!uint(owner.epoch)||!uint(owner.generation)){
    throw new Error('Invalid Workspace terminal attachment.');
  }
  return identity;
}

export function installWorkspace(document,invoke){
  if(!invoke)return;
  const byId=id=>document.getElementById(id);
  const root=byId('agent-workspace'),refresh=byId('workspace-refresh'),more=byId('workspace-more');
  const previous=byId('workspace-previous');
  const select=byId('workspace-conversation'),attach=byId('workspace-attach'),detach=byId('workspace-detach');
  const selection=byId('workspace-selection'),status=byId('workspace-status'),panel=byId('workspace-view');
  const canvas=byId('workspace-canvas'),input=byId('workspace-input'),output=byId('workspace-output');
  const encoder=new TextEncoder();
  let request=0,loading=false,attempt=null,opening=false,active=null,detaching=false;
  let pages=[],pageIndex=-1,evictedOlderPages=false;
  const page=()=>pages[pageIndex];

  const ime=installIme(input,{getEpoch:()=>!input.disabled?active:null,commit:(text,view)=>{
    if(text.length<=4096&&encoder.encode(text).length<=4096){
      enqueue(view,{operation:'text',text});
    }
  }});

  async function call(command,args,id=++request){
    if(!uint(id))throw new Error('Workspace request counter exhausted.');
    const receipt=await invoke(command,{request:id,...args});
    if(receipt?.request!==id)throw new Error('Stale Workspace response.');
    return receipt.value;
  }

  const selected=()=>page()?.receipt.rows.find(row=>row.id===select.value);
  function controls(){
    const changing=loading||opening||detaching;
    refresh.disabled=changing||!!active;
    previous.disabled=changing||!!active||pageIndex<1;
    more.disabled=changing||!!active||!(pageIndex+1<pages.length||page()?.receipt.next_before);
    select.disabled=changing||!!active||!page()?.receipt.rows.length;
    attach.disabled=changing||!!active||!selected()?.reopenable;
    detach.disabled=changing||!active;
    input.disabled=changing||!active?.writable;
  }

  function showSelection(){
    const row=selected();
    selection.textContent=row?[`${row.provider} · ${row.status}`,row.directory].filter(Boolean).join(' · '):'';
    attempt=null;
    controls();
  }

  function displayPage(){
    const current=page();
    const options=current.receipt.rows.map(row=>{
      const option=document.createElement('option');
      option.value=row.id;option.textContent=row.title||'Untitled conversation';
      return option;
    });
    select.replaceChildren(...options);
    if(options.some(option=>option.value===current.selection))select.value=current.selection;
    status.textContent=options.length?'':current.receipt.next_before
      ?'No local CLI sessions on this page. Use Next page.'
      :'No CLI conversations in this part of the shared catalog.';
    if(pageIndex===0&&evictedOlderPages){
      status.textContent=[status.textContent,'Earlier pages were released. Refresh to start over.'].filter(Boolean).join(' ');
    }
    showSelection();
  }

  async function load(cursor=null){
    if(loading||active||opening||detaching)return;
    const oldSelection=cursor?'':select.value;
    loading=true;controls();status.textContent='Reading the shared Workspace catalog…';
    try{
      const next=validateWorkspacePage(await call('workspace_catalog',{cursor}));
      if(cursor&&(next.generation!==cursor.generation||
          next.next_before!==null&&BigInt(next.next_before)>=BigInt(cursor.before))){
        throw new Error('Workspace page changed.');
      }
      if(!cursor){pages=[];evictedOlderPages=false;}
      pages.push({receipt:next,selection:oldSelection});
      if(pages.length>CATALOG_PAGE_MEMORY){pages.shift();evictedOlderPages=true;}
      pageIndex=pages.length-1;
      displayPage();
    }catch(error){
      // A failed or lost request can already have advanced the shared
      // catalog generation or server cursor. Retire the local navigation
      // receipts instead of replaying the same cursor or allowing attachment.
      pages=[];pageIndex=-1;evictedOlderPages=false;
      select.replaceChildren();showSelection();
      status.textContent=`${String(error?.message??error)} Refresh the catalog to continue.`;
    }finally{loading=false;controls();}
  }

  function turnPage(change){
    if(loading||active||opening||detaching||!page())return;
    page().selection=select.value;
    const target=pageIndex+change;
    if(target>=0&&target<pages.length){
      pageIndex=target;displayPage();
    }else if(change===1&&page().receipt.next_before){
      const {generation,next_before:before}=page().receipt;
      void load({generation,before});
    }
  }

  function operate(view,operation){return call('workspace_terminal',{identity:view.identity,operation});}
  async function acknowledge(view,revision){
    const result=await operate(view,{operation:'ack',revision});
    if(result?.kind!=='receipt')throw new Error('Invalid Workspace frame acknowledgement.');
  }

  async function start(){
    const row=selected();
    if(attach.disabled||!row)return;
    const launch=attempt??{id:row.id,generation:page().receipt.generation,request:++request};
    // Async attach may complete after the user has started working elsewhere.
    const attachFocus=element=>element===attach||element===document.body;
    const pageWindow=document.defaultView;
    let focusRequested=attachFocus(document.activeElement)&&document.hasFocus()&&!document.hidden;
    const retireFocus=()=>{focusRequested=false;};
    const observeFocus=event=>{if(!attachFocus(event.target))retireFocus();};
    const observeVisibility=()=>{if(document.hidden)retireFocus();};
    if(focusRequested){
      document.addEventListener('focusin',observeFocus,true);
      document.addEventListener('visibilitychange',observeVisibility);
      pageWindow?.addEventListener('blur',retireFocus);
    }
    attempt=launch;opening=true;controls();status.textContent='Requesting the Workspace terminal lease…';
    let view=null;
    try{
      const receipt=await call('workspace_attach',{
        conversation:launch.id,catalogGeneration:launch.generation,
      },launch.request);
      const identity=validateWorkspaceAttachment(receipt,launch.id);
      view={identity,frame:null,chain:Promise.resolve(),pending:0,writable:false,locked:false,polling:false,timer:null};
      view.frame=acceptFull(null,receipt.frame,identity.owner);
      paint(canvas,view.frame);
      active=view;panel.hidden=false;
      output.value='';output.hidden=true;
      await acknowledge(view,view.frame.revision);
      if(active!==view)return;
      attempt=null;
      view.timer=setInterval(()=>void poll(view),50);
      await poll(view);
    }catch(error){
      if(view){
        active=null;panel.hidden=true;clearPaint(canvas);attempt=null;
        try{await operate(view,{operation:'detach'});}catch{/* The host closes lost leases. */}
      }
      status.textContent=String(error?.message??error);
    }finally{
      document.removeEventListener('focusin',observeFocus,true);
      document.removeEventListener('visibilitychange',observeVisibility);
      pageWindow?.removeEventListener('blur',retireFocus);
      opening=false;controls();
      if(!input.disabled&&focusRequested&&root.open&&document.hasFocus()&&!document.hidden&&
          attachFocus(document.activeElement))input.focus();
    }
  }

  async function poll(view){
    if(active!==view||view.polling||detaching)return;
    view.polling=true;
    try{
      const result=await operate(view,{operation:'poll'});
      if(active!==view||detaching)return;
      const update=result?.update;
      if(result?.kind!=='polled'||!boundedText(update?.status,8192)||typeof update.writable!=='boolean'){
        throw new Error('Invalid Workspace terminal update.');
      }
      if(update.frame!==null){
        const next=acceptUpdate(view.frame,update.frame,view.identity.owner);
        paint(canvas,next);view.frame=next;
        await acknowledge(view,next.revision);
        if(active!==view||detaching)return;
      }
      const wasWritable=view.writable;
      view.writable=update.writable&&!view.locked;
      if(!view.locked)status.textContent=update.status;
      if(!update.writable){
        if(wasWritable)ime.reset();
        clearInterval(view.timer);
      }
    }catch(error){
      if(active===view&&!detaching){
        view.writable=false;view.locked=true;ime.reset();clearInterval(view.timer);
        status.textContent=`Workspace output interrupted. Detach and attach to recover. ${String(error?.message??error)}`;
      }
    }finally{view.polling=false;controls();}
  }

  function enqueue(view,operation){
    if(!view||active!==view||input.disabled)return;
    if(view.pending>=128){
      view.writable=false;view.locked=true;controls();
      status.textContent='Workspace keyboard queue is full. Detach and attach to recover.';
      return;
    }
    view.pending++;
    view.chain=view.chain.then(async()=>{
      if(active!==view||!view.writable||detaching)return;
      try{
        const result=await operate(view,operation);
        if(result?.kind!=='receipt')throw new Error('Invalid Workspace input receipt.');
      }catch(error){
        if(active!==view||detaching)return;
        view.writable=false;view.locked=true;ime.reset();controls();
        status.textContent=`Workspace input interrupted. Detach and attach to recover. ${String(error?.message??error)}`;
      }
    }).finally(()=>{view.pending--;});
  }

  async function release(){
    const view=active;
    if(!view||detach.disabled)return;
    detaching=true;controls();ime.reset();
    clearInterval(view.timer);
    try{
      const response=await operate(view,{operation:'detach'});
      if(response?.kind!=='receipt')throw new Error('Invalid Workspace detach receipt.');
      active=null;output.value='';output.hidden=true;panel.hidden=true;clearPaint(canvas);
      status.textContent='Terminal detached. The Workspace host retains the provider session.';
    }catch(error){
      view.writable=false;view.locked=true;
      status.textContent=`Detach unconfirmed. ${String(error?.message??error)}`;
    }finally{detaching=false;controls();}
  }

  root.hidden=false;
  root.addEventListener('toggle',()=>{if(root.open&&!page()&&!loading)void load();});
  refresh.addEventListener('click',()=>void load());
  previous.addEventListener('click',()=>turnPage(-1));
  more.addEventListener('click',()=>turnPage(1));
  select.addEventListener('change',()=>{status.textContent='';showSelection();});
  attach.addEventListener('click',start);
  detach.addEventListener('click',release);
  canvas.addEventListener('click',()=>input.focus());
  input.addEventListener('paste',event=>event.preventDefault());
  input.addEventListener('keydown',event=>{
    if(event.defaultPrevented||input.disabled||ime.isComposing()||event.isComposing||
        event.keyCode===229||event.metaKey||event.getModifierState?.('AltGraph')||
        event.key==='Dead'||event.key==='Process')return;
    if(!keys.test(event.key)&&(!(event.ctrlKey||event.altKey)||[...event.key].length!==1))return;
    event.preventDefault();
    enqueue(active,{operation:'key',name:event.key,shift:event.shiftKey,alt:event.altKey,ctrl:event.ctrlKey});
  });
  byId('workspace-copy-visible').addEventListener('click',()=>{
    if(!active?.frame)return;
    output.value=active.frame.rows.map(row=>row.filter(cell=>cell.width!==0)
      .map(cell=>cell.text).join('').replace(/ +$/,'')).join('\n');
    output.hidden=false;
  });
  controls();
}

if(typeof document!=='undefined')installWorkspace(document,globalThis.__TAURI__?.core?.invoke);
