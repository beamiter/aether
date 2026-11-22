const fields=['goal','completed','remaining','constraints'];
const bounded=(value,limit)=>typeof value==='string'&&value.length<=limit;
const revision=value=>typeof value==='string'&&/^(0|[1-9][0-9]{0,19})$/.test(value);
const sameHandoff=(a,b)=>a?.revision===b?.revision&&a?.title===b?.title&&
  a?.repository===b?.repository&&a?.body===b?.body;

export function installWorkspaceContext(document,{owner,sameIdentity,call,work,canDraft,openDraft}){
  const byId=id=>document.getElementById('native-context-'+id);
  const root=document.getElementById('native-context'),status=byId('status');
  const inputs=Object.fromEntries(fields.map(field=>[field,byId(field)]));
  const note=byId('note'),materials=byId('materials'),packet=byId('packet'),body=byId('body');
  const reload=byId('reload'),save=byId('save'),preview=byId('preview'),use=byId('use');
  let saved=null,reviewed=null,busy=false;

  function dirty(){
    return !!saved&&(fields.some(field=>inputs[field].value!==saved.fields[field])||!!note.value.trim());
  }
  function controls(nextBusy=busy){
    busy=nextBusy;
    const editing=dirty();
    reload.disabled=busy||!owner();
    reload.textContent=editing?'Discard edits and reload':'Reload saved context';
    save.disabled=busy||!editing;
    preview.disabled=busy||!saved||editing||!saved.fields.goal.trim();
    use.disabled=busy||!reviewed||editing;
    for(const input of Object.values(inputs))input.disabled=busy||!saved;
    note.disabled=busy||!saved||saved.materials.length>=16;
  }
  function clearPreview(){
    reviewed=null;packet.hidden=true;body.textContent='';
  }
  function reset(){
    saved=null;clearPreview();root.open=false;
    status.textContent='';note.value='';materials.replaceChildren();
    for(const input of Object.values(inputs))input.value='';
    controls();
  }
  function contextWork(run){
    const identity=owner();
    if(!identity)return;
    return work(async()=>{
      if(!sameIdentity(identity,owner()))return;
      try{await run(identity);}
      catch(error){if(sameIdentity(identity,owner()))status.textContent=String(error?.message??error);}
    });
  }
  async function request(identity,choice,kind){
    const reply=await call({operation:'context',identity,choice});
    const key={context:'context',context_preview:'preview',context_draft:'draft'}[kind];
    const result=reply?.[key];
    if(reply?.kind!==kind||!sameIdentity(identity,owner())||
       !sameIdentity(result?.identity,identity)||!revision(result.revision)){
      throw new Error('Missing current task context receipt.');
    }
    return result;
  }
  function display(value){
    if(!fields.every(field=>bounded(value.fields?.[field],65536))||
       !Array.isArray(value.materials)||value.materials.length>16||
       !value.materials.every(item=>bounded(item?.label,256)&&bounded(item.kind,64)&&bounded(item.text,65536))){
      throw new Error('Invalid shared context projection.');
    }
    saved=value;
    for(const field of fields)inputs[field].value=value.fields[field];
    note.value='';
    materials.replaceChildren(...value.materials.map(item=>{
      const detail=document.createElement('details');
      const label=document.createElement('summary'),text=document.createElement('pre');
      label.textContent=item.label||item.kind;
      text.textContent=item.text;detail.append(label,text);
      return detail;
    }));
  }
  function handoff(value,currentRevision){
    if(value.revision!==currentRevision||!bounded(value.title,8192)||
       !bounded(value.repository,4096)||!bounded(value.body,16384)||!value.body.trim()){
      throw new Error('Invalid current task handoff.');
    }
    return value;
  }
  async function load(identity){
    clearPreview();
    const value=await request(identity,{kind:'read'},'context');
    if(saved&&BigInt(value.revision)<BigInt(saved.revision))throw new Error('Stale shared context.');
    display(value);
    status.textContent=`Shared context revision ${value.revision}.`;
  }
  async function saveContext(identity){
    const old=saved.revision;
    clearPreview();
    const value=await request(identity,{
      kind:'save',revision:old,fields:Object.fromEntries(fields.map(field=>[field,inputs[field].value])),
      note:note.value.trim()?note.value:null,
    },'context');
    if(BigInt(value.revision)!==BigInt(old)+1n)throw new Error('Unexpected context save revision.');
    display(value);
    status.textContent=`Context saved at revision ${value.revision}.`;
  }
  async function prepare(identity){
    clearPreview();
    const current=saved.revision;
    reviewed=handoff(await request(identity,{kind:'preview',revision:current},'context_preview'),current);
    body.textContent=reviewed.body;
    packet.hidden=false;
    status.textContent=`Codex handoff preview of revision ${reviewed.revision}.`;
  }
  async function usePreview(identity){
    if(!canDraft())throw new Error('A task draft already exists. Keep it or clear it before opening a handoff.');
    const previous=reviewed;
    clearPreview();
    const draft=handoff(await request(identity,{kind:'use',revision:previous.revision},'context_draft'),previous.revision);
    if(!sameHandoff(previous,draft))throw new Error('The reviewed handoff changed.');
    openDraft(draft);
  }

  root.addEventListener('toggle',()=>{if(root.open&&!saved)void contextWork(load);});
  reload.addEventListener('click',()=>contextWork(load));
  save.addEventListener('click',()=>contextWork(saveContext));
  preview.addEventListener('click',()=>contextWork(prepare));
  use.addEventListener('click',()=>contextWork(usePreview));
  for(const input of [note,...Object.values(inputs)]){
    input.addEventListener('input',()=>{clearPreview();controls();});
  }
  return {controls,reset};
}
