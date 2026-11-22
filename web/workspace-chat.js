const decimal=value=>typeof value==='string'&&/^[0-9]{1,20}$/.test(value);
const chatId=id=>typeof id==='string'&&id.startsWith('jterm:api:')&&id.length<=4096;
const bounded=(value,max)=>typeof value==='string'&&value.length<=max;
const live=job=>['Starting','Running','Stopping'].includes(job?.state);
const states=new Set(['Starting','Running','Stopping','Completed','Cancelled','Failed','Interrupted']);
const same=(a,b)=>a?.conversation===b?.conversation&&a?.generation===b?.generation;
const encoder=new TextEncoder();

export function validateChatJob(job,previous=null){
  if(!job||!decimal(job.epoch)||!decimal(job.sequence)||!states.has(job.state)||
     !bounded(job.user_text,65536)||!bounded(job.preview,65536)||typeof job.can_cancel!=='boolean'||
     (job.can_cancel&&!['Starting','Running'].includes(job.state))){
    throw new Error('Invalid API request snapshot.');
  }
  if(previous&&(BigInt(job.epoch)<BigInt(previous.epoch)||
     (job.epoch===previous.epoch&&BigInt(job.sequence)<BigInt(previous.sequence)))){
    throw new Error('Stale API request snapshot.');
  }
  return job;
}

export function validateChatView(chat,expected=null){
  const identity=chat?.identity;
  if(!chatId(identity?.conversation)||!Number.isInteger(identity.generation)||identity.generation<=0||
     identity.generation>0xffffffff||(expected&&!same(identity,expected))||
     !decimal(chat.revision)||!decimal(chat.draft_revision)||!bounded(chat.draft,65536)||
     !bounded(chat.title,8192)||typeof chat.archived!=='boolean'||
     typeof chat.background_supported!=='boolean'||typeof chat.terminal_context!=='boolean'||typeof chat.has_older!=='boolean'||
     !Array.isArray(chat.turns)||chat.turns.length>100||
     chat.turns.some(turn=>!['user','assistant'].includes(turn?.role)||!bounded(turn.text,262144))){
    throw new Error('Invalid API conversation view.');
  }
  if(chat.job!==null)validateChatJob(chat.job);
  return chat;
}

export function installWorkspaceChat(document,invoke){
  if(!invoke)return;
  const byId=id=>document.getElementById(id);
  const root=byId('workspace-api'),panel=byId('chat-view');
  const select=byId('chat-select'),open=byId('chat-open'),create=byId('chat-new');
  const refresh=byId('chat-refresh'),more=byId('chat-more'),detach=byId('chat-detach');
  const draft=byId('chat-draft'),save=byId('chat-save'),send=byId('chat-send'),reload=byId('chat-reload');
  const history=byId('chat-history'),livePanel=byId('chat-live'),status=byId('chat-status');
  const jobStatus=byId('chat-job-status'),prompt=byId('chat-job-prompt'),preview=byId('chat-preview');
  const cancel=byId('chat-cancel');
  let request=0,active=null,busy=false,detaching=false,cursor=null,catalogLoaded=false;

  async function call(action){
    const id=++request;
    if(id>0xffffffff)throw new Error('API conversation request counter exhausted.');
    const receipt=await invoke('workspace_chat',{request:id,action});
    if(receipt?.request!==id)throw new Error('Stale API conversation receipt.');
    return receipt.value;
  }

  function controls(){
    refresh.disabled=create.disabled=busy||!!active;
    more.disabled=refresh.disabled||!cursor;
    select.disabled=refresh.disabled||!select.options.length;
    open.disabled=select.disabled||!select.value;
    detach.disabled=reload.disabled=busy||!active;
    draft.disabled=!active||active.server.archived||detaching;
    save.disabled=busy||draft.disabled||active.edited===active.saved;
    send.disabled=busy||draft.disabled||!active.server.background_supported||!draft.value.trim()||live(active.job);
    cancel.disabled=busy||!active?.job?.can_cancel;
  }

  function work(task){
    if(busy)return;
    busy=true;controls();
    return task().catch(error=>{status.textContent=String(error?.message??error);})
      .finally(()=>{busy=false;controls();});
  }

  function node(tag,text){
    const item=document.createElement(tag);
    if(text!==undefined)item.textContent=text;
    return item;
  }

  async function list(before=null){
    const reply=await call({operation:'list',before});
    if(reply?.kind!=='catalog'||!Array.isArray(reply.rows)||reply.rows.length>32||
       !(reply.next_before===null||decimal(reply.next_before))||
       reply.rows.some(row=>!chatId(row?.id)||!bounded(row.title,8192))){
      throw new Error('Invalid API conversation catalog receipt.');
    }
    cursor=reply.next_before;catalogLoaded=true;
    const options=reply.rows.map(row=>{
      const option=node('option',row.title||'Untitled conversation');option.value=row.id;
      return option;
    });
    select.replaceChildren(...options);
    status.textContent=options.length?'':cursor?'Use Next page to continue.':'No API conversations on this page.';
  }

  function renderJob(view){
    const job=view.job;
    livePanel.hidden=!job;
    if(!job)return;
    const label={
      Starting:'Request accepted by the Workspace host.',
      Running:'Streaming from the Workspace host.',
      Stopping:'Waiting for the host to stop the request.',
      Completed:'Request completed.',
      Cancelled:'Request cancelled.',
      Failed:'API request failed.',
      Interrupted:'API request was interrupted.',
    }[job.state];
    if(jobStatus.textContent!==label)jobStatus.textContent=label;
    const userText=live(job)?job.user_text:'',previewText=live(job)?job.preview:'';
    if(prompt.textContent!==userText)prompt.textContent=userText;
    if(preview.textContent!==previewText)preview.textContent=previewText;
  }

  function applyChat(view,chat,replaceDraft){
    if(active!==view)return;
    if(view.server&&BigInt(chat.revision)<BigInt(view.server.revision))throw new Error('Stale API conversation revision.');
    if(!view.server||chat.revision!==view.server.revision){
      byId('chat-title').textContent=chat.title;
      byId('chat-older').hidden=!chat.has_older;
      byId('chat-context').hidden=!chat.terminal_context;
    }
    const previous=view.server?.turns;
    if(previous?.length!==chat.turns.length||chat.turns.some((turn,index)=>
      turn.role!==previous[index].role||turn.text!==previous[index].text)){
      history.replaceChildren(...chat.turns.map(turn=>{
        const article=node('article');
        article.append(node('strong',turn.role==='user'?'You':'Assistant'),node('pre',turn.text));
        return article;
      }));
    }
    view.server=chat;
    if(chat.job)view.job=validateChatJob(chat.job,view.job);
    if(replaceDraft)draft.value=chat.draft;
    renderJob(view);
  }

  async function choose(action){
    status.textContent='Opening from the Workspace host…';
    const reply=await call(action);
    if(reply?.kind!=='chat')throw new Error('Missing API conversation receipt.');
    const chat=validateChatView(reply.chat);
    if(action.conversation&&chat.identity.conversation!==action.conversation){
      throw new Error('API conversation selection changed.');
    }
    const view={identity:chat.identity,server:null,job:null,edited:0,saved:0,polling:false,
      refreshPending:!!chat.job&&!live(chat.job)};
    active=view;
    applyChat(view,chat,true);
    panel.hidden=false;
    view.timer=setInterval(()=>void poll(view),200);
    status.textContent='';
  }

  async function acknowledgeDraft(view,text,edited){
    if(text.length>65536||encoder.encode(text).length>65536)throw new Error('The draft exceeds 64 KiB.');
    const reply=await call({operation:'save',identity:view.identity,revision:view.server.draft_revision,text});
    if(active!==view)return;
    if(reply?.kind!=='chat')throw new Error('Missing durable draft receipt.');
    const chat=validateChatView(reply.chat,view.identity);
    view.saved=edited;
    applyChat(view,chat,view.edited===edited);
    status.textContent=view.edited===edited?'Draft saved.':'Earlier draft saved. New edits are pending.';
  }

  async function reloadChat(view,background=false){
    const reply=await call({operation:'reload',identity:view.identity});
    if(active!==view||(background&&busy))return;
    if(reply?.kind!=='chat')throw new Error('Missing API conversation reload receipt.');
    applyChat(view,validateChatView(reply.chat,view.identity),view.edited===view.saved);
    view.refreshPending=false;
  }

  async function submit(){
    const view=active,text=draft.value,edited=view.edited;
    if(view.edited!==view.saved)await acknowledgeDraft(view,text,edited);
    if(active!==view||view.edited!==edited){
      status.textContent='The draft changed while saving. Select Send message for the updated draft.';
      return;
    }
    status.textContent='Waiting for Workspace host admission…';
    const reply=await call({
      operation:'send',identity:view.identity,text,
      revision:view.server.revision,draft_revision:view.server.draft_revision,
    });
    if(active!==view)return;
    if(reply?.kind!=='job'||!same(reply.identity,view.identity))throw new Error('Missing API execution receipt.');
    view.job=validateChatJob(reply.job,view.job);
    // Admission consumed exactly this persisted revision. Never clear a
    // later edit, even if it contains the same text.
    if(view.edited===edited)draft.value='';
    view.refreshPending=!live(view.job);
    renderJob(view);
    status.textContent='Request admitted by the Workspace host.';
  }

  async function poll(view){
    if(busy||view.polling||active!==view||(!live(view.job)&&!view.refreshPending))return;
    view.polling=true;
    try{
      if(view.refreshPending){
        await reloadChat(view,true);
        return;
      }
      const reply=await call({operation:'observe',identity:view.identity});
      if(active!==view||busy)return;
      if(reply?.kind!=='job'||!same(reply.identity,view.identity))throw new Error('Invalid API observation receipt.');
      view.job=validateChatJob(reply.job,view.job);
      renderJob(view);
      if(!live(view.job)){
        view.refreshPending=true;
        await reloadChat(view,true);
      }
    }catch(error){
      if(active===view&&!busy)status.textContent=String(error?.message??error);
    }finally{view.polling=false;controls();}
  }

  async function release(){
    const view=active;
    if(view.edited!==view.saved)await acknowledgeDraft(view,draft.value,view.edited);
    if(view.edited!==view.saved){
      status.textContent='Save your newer edits before detaching.';return;
    }
    const reply=await call({operation:'detach',identity:view.identity});
    if(reply?.kind!=='ack')throw new Error('API conversation detach could not be confirmed.');
    clearInterval(view.timer);active=null;panel.hidden=true;history.replaceChildren();draft.value='';
    prompt.textContent=preview.textContent=jobStatus.textContent='';
    status.textContent='Conversation detached.';
  }

  root.hidden=false;
  root.addEventListener('toggle',()=>{if(root.open&&!catalogLoaded&&!active)void work(()=>list());});
  refresh.addEventListener('click',()=>work(()=>list()));
  more.addEventListener('click',()=>work(()=>list(cursor)));
  select.addEventListener('change',controls);
  open.addEventListener('click',()=>work(()=>choose({operation:'open',conversation:select.value})));
  create.addEventListener('click',()=>work(()=>choose({operation:'create'})));
  save.addEventListener('click',()=>work(()=>acknowledgeDraft(active,draft.value,active.edited)));
  reload.addEventListener('click',()=>work(async()=>{await reloadChat(active);status.textContent='Conversation reloaded.';}));
  detach.addEventListener('click',()=>work(async()=>{
    detaching=true;controls();
    try{await release();}finally{detaching=false;}
  }));
  draft.addEventListener('input',()=>{if(active){active.edited++;status.textContent='Draft has unsaved edits.';controls();}});
  byId('chat-form').addEventListener('submit',event=>{
    event.preventDefault();
    if(!send.disabled)void work(submit);
  });
  cancel.addEventListener('click',()=>work(async()=>{
    const view=active;
    const reply=await call({operation:'cancel',identity:view.identity});
    if(active!==view)return;
    if(reply?.kind!=='job'||!same(reply.identity,view.identity))throw new Error('Missing API cancellation receipt.');
    view.job=validateChatJob(reply.job,view.job);
    view.refreshPending=!live(view.job);
    renderJob(view);
    status.textContent='Cancellation accepted by the Workspace host.';
  }));
  controls();
}

if(typeof document!=='undefined')installWorkspaceChat(document,globalThis.__TAURI__?.core?.invoke);
