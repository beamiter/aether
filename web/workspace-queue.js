const bounded=(value,limit)=>typeof value==='string'&&value.length<=limit;
const taskId=value=>bounded(value,32768)&&value.startsWith('jterm:task:v1:');
const revision=value=>typeof value==='string'&&/^(0|[1-9][0-9]{0,19})$/.test(value);
const same=(a,b)=>a?.task===b?.task&&a?.incarnation===b?.incarnation&&a?.generation===b?.generation;

export function installWorkspaceQueue(document,{invoke,source,launch}){
  const byId=id=>document.getElementById('queue-'+id);
  const root=document.getElementById('workspace-queue'),status=byId('status'),items=byId('entries');
  const refresh=byId('refresh'),selected=byId('selected'),name=byId('task-title');
  const dependencies=byId('dependencies'),save=byId('save'),remove=byId('remove');
  const launchControls=byId('launch-controls'),confirm=byId('confirm'),start=byId('launch');
  let request=0,busy=false,nativeBusy=false,view=null,selectedIdentity=null;
  const selectedRow=()=>view?.rows.find(row=>row.id===selectedIdentity?.task);
  const picked=()=>Array.from(dependencies.selectedOptions,option=>option.value);

  function dirty(){
    const initial=selectedRow()?.dependencies??[];
    const next=picked();
    return initial.length!==next.length||next.some(id=>!initial.includes(id));
  }

  function node(tag,text){
    const element=document.createElement(tag);
    if(text!==undefined)element.textContent=text;
    return element;
  }

  function fillDependencies(){
    const prior=selectedRow()?.dependencies??[];
    const options=(view?.tasks??[]).filter(task=>task.id!==selectedIdentity?.task);
    const retained=prior.filter(id=>!options.some(task=>task.id===id)).map(id=>({id,title:'Unavailable task'}));
    dependencies.replaceChildren(...options.concat(retained).map(task=>{
      const option=node('option',task.title||'Untitled task');
      option.value=task.id;option.selected=prior.includes(task.id);
      return option;
    }));
  }

  function controls(working=nativeBusy){
    nativeBusy=working;
    const task=source(),next=task?.identity??null;
    if(!same(next,selectedIdentity)){
      selectedIdentity=next;
      confirm.checked=false;
      fillDependencies();
    }
    const row=selectedRow();
    selected.hidden=!task;
    name.textContent=task?.title??'';
    refresh.disabled=busy;
    dependencies.disabled=busy||nativeBusy||!task||!view;
    save.disabled=dependencies.disabled||!!row&&!dirty();
    remove.disabled=dependencies.disabled||!row;
    launchControls.hidden=!task||task.draft_goal===null;
    confirm.disabled=busy||nativeBusy||!task?.controlled||!row?.ready||dirty()||launchControls.hidden;
    start.disabled=confirm.disabled||!confirm.checked;
  }

  function work(action){
    if(busy)return;
    busy=true;controls();
    return action().catch(error=>{status.textContent=String(error?.message??error);})
      .finally(()=>{busy=false;controls();});
  }

  async function call(choice){
    const id=++request;
    if(id>0xffffffff)throw new Error('Queue request counter exhausted.');
    const receipt=await invoke('workspace_queue',{request:id,choice});
    const next=receipt?.value;
    if(receipt?.request!==id||!revision(next?.revision)||!Array.isArray(next.rows)||next.rows.length>256||
       !Array.isArray(next.tasks)||next.tasks.length>4096||
       !next.tasks.every(item=>taskId(item?.id)&&bounded(item.title,8192))||
       !next.rows.every(row=>taskId(row?.id)&&bounded(row.title,8192)&&bounded(row.state,128)&&
         typeof row.ready==='boolean'&&(row.reason===null||bounded(row.reason,4096))&&
         bounded(row.location,1024)&&Array.isArray(row.dependencies)&&row.dependencies.length<=32&&
         row.dependencies.every(taskId))){
      throw new Error('Invalid shared task queue receipt.');
    }
    return next;
  }

  function display(next,message){
    view=next;
    confirm.checked=false;
    const titles=new Map(next.tasks.map(task=>[task.id,task.title]));
    items.replaceChildren(...next.rows.map(row=>{
      const entry=node('li');
      entry.dataset.task=row.id;
      const state=node('strong',row.title);
      entry.append(state,document.createTextNode(` · ${row.state} · ${row.location}`));
      if(row.dependencies.length){
        entry.append(node('p',`Dependencies · ${row.dependencies.map(id=>titles.get(id)??'Unavailable task').join(', ')}`));
      }
      if(row.reason)entry.append(node('p',row.reason));
      return entry;
    }));
    fillDependencies();
    status.textContent=`${message} · revision ${next.revision}.`;
  }

  async function read(){
    const next=await call({operation:'read'});
    if(view&&BigInt(next.revision)<BigInt(view.revision))throw new Error('Stale queue snapshot.');
    display(next,next.rows.length?'Queue refreshed':'Queue is empty');
  }

  async function edit(operation){
    const source=selectedIdentity,current=view.revision;
    const choice={operation,source,revision:current};
    if(operation==='save')choice.dependencies=picked();
    const next=await call(choice);
    if(BigInt(next.revision)!==BigInt(current)+1n)throw new Error('Queue update was not confirmed.');
    display(next,operation==='save'?'Queue plan saved':'Task removed from queue');
  }

  root.hidden=false;
  root.addEventListener('toggle',()=>{if(root.open&&!view)void work(read);});
  refresh.addEventListener('click',()=>work(read));
  dependencies.addEventListener('change',()=>{confirm.checked=false;controls();});
  confirm.addEventListener('change',()=>controls());
  save.addEventListener('click',()=>work(()=>edit('save')));
  remove.addEventListener('click',()=>work(()=>edit('remove')));
  start.addEventListener('click',()=>{
    if(start.disabled)return;
    const expected=view.revision,identity=selectedIdentity;
    confirm.checked=false;
    void work(async()=>{
      await launch(identity,expected);
      await read();
    });
  });
  controls();
  return {controls};
}
