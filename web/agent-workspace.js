import {acceptFull,acceptUpdate,clearPaint,paint} from './frames.js';
import {installIme} from './ime.js';

const uint=value=>Number.isInteger(value)&&value>0&&value<=0xffffffff;
const conversationId=value=>typeof value==='string'&&value.startsWith('jterm:cli:v1:')&&value.length<=32768;
const boundedText=(value,limit)=>typeof value==='string'&&value.length<=limit;
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
  const select=byId('workspace-conversation'),attach=byId('workspace-attach'),detach=byId('workspace-detach');
  const selection=byId('workspace-selection'),status=byId('workspace-status'),panel=byId('workspace-view');
  const canvas=byId('workspace-canvas'),input=byId('workspace-input'),output=byId('workspace-output');
  const encoder=new TextEncoder();
  let request=0,catalog=null,loading=false,attempt=null,opening=false,active=null,detaching=false;

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

  const selected=()=>catalog?.rows.find(row=>row.id===select.value);
  function controls(){
    const changing=loading||opening||detaching;
    refresh.disabled=changing||!!active;
    more.disabled=changing||!!active||!catalog?.next_before;
    select.disabled=changing||!!active||!catalog?.rows.length;
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

  async function load(cursor=null){
    if(loading||active||opening)return;
    const oldSelection=select.value;
    loading=true;controls();status.textContent='Reading the shared Workspace catalog…';
    try{
      const next=validateWorkspacePage(await call('workspace_catalog',{cursor}));
      if(cursor&&next.generation!==cursor.generation)throw new Error('Workspace page changed.');
      catalog=next;
      const options=catalog.rows.map(row=>{
        const option=document.createElement('option');
        option.value=row.id;option.textContent=row.title||'Untitled conversation';
        return option;
      });
      select.replaceChildren(...options);
      if(options.some(option=>option.value===oldSelection))select.value=oldSelection;
      status.textContent=options.length?'':catalog.next_before
        ?'No local CLI sessions on this page. Use Next page.'
        :'No CLI conversations in this part of the shared catalog.';
      showSelection();
    }catch(error){status.textContent=String(error?.message??error);}
    finally{loading=false;controls();}
  }

  function operate(view,operation){return call('workspace_terminal',{identity:view.identity,operation});}
  async function acknowledge(view,revision){
    const result=await operate(view,{operation:'ack',revision});
    if(result?.kind!=='receipt')throw new Error('Invalid Workspace frame acknowledgement.');
  }

  async function start(){
    const row=selected();
    if(attach.disabled||!row)return;
    const launch=attempt??{id:row.id,generation:catalog.generation,request:++request};
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
      opening=false;controls();
      if(active?.writable)input.focus();
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
  root.addEventListener('toggle',()=>{if(root.open&&!catalog&&!loading)void load();});
  refresh.addEventListener('click',()=>void load());
  more.addEventListener('click',()=>void load({generation:catalog.generation,before:catalog.next_before}));
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
