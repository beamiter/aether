const bounded=(value,limit)=>typeof value==='string'&&value.length<=limit;
const cursor=value=>typeof value==='string'&&/^[1-9][0-9]{0,19}$/.test(value);
const sameScope=(left,right)=>left?.task===right?.task&&left?.incarnation===right?.incarnation&&
  left?.generation===right?.generation;
const sameAnchor=(left,right)=>left?.conversation===right?.conversation&&left?.event_id===right?.event_id&&
  left?.revision===right?.revision;
const validAnchor=value=>bounded(value?.conversation,32768)&&!!value.conversation&&
  bounded(value.event_id,512)&&!!value.event_id&&bounded(value.revision,256)&&!!value.revision;

export function installWorkspaceTools(document,{invoke,source}){
  const byId=id=>document.getElementById('tools-'+id);
  const root=document.getElementById('workspace-tools'),form=byId('search'),status=byId('status');
  const query=byId('query'),scoped=byId('scope'),archived=byId('archived'),submit=byId('submit');
  const results=byId('results'),coverage=byId('coverage'),more=byId('more'),panel=byId('document');
  const evidenceSource=byId('document-source'),identity=byId('document-identity'),body=byId('document-text');
  let request=0,generation=0,intent=0,busy=false,page=null,previousScope=null;
  const scope=()=>scoped.checked?(source()?.identity??null):null;

  function clearDocument(){
    panel.hidden=true;evidenceSource.textContent=identity.textContent=body.textContent='';
    delete identity.dataset.conversation;
    delete identity.dataset.eventId;
    delete identity.dataset.revision;
  }

  function retire(){
    intent++;page=null;results.replaceChildren();coverage.textContent=status.textContent='';
    clearDocument();
  }

  function controls(){
    const active=source();
    if(!active)scoped.checked=false;
    const next=scope();
    if(!sameScope(previousScope,next)){
      previousScope=next;retire();
    }
    query.disabled=archived.disabled=busy;
    scoped.disabled=busy||!active;
    submit.disabled=busy||!query.value.trim();
    more.disabled=busy||!page?.next_before;
    for(const button of results.querySelectorAll('button'))button.disabled=busy;
  }

  function work(action){
    if(busy)return;
    busy=true;controls();
    const selectedIntent=intent;
    return action(selectedIntent).catch(error=>{
      if(selectedIntent===intent)status.textContent=String(error?.message??error);
    }).finally(()=>{busy=false;controls();});
  }

  async function call(choice){
    const id=++request;
    if(id>0xffffffff)throw new Error('Tools request counter exhausted.');
    const receipt=await invoke('workspace_tools',{request:id,choice});
    if(receipt?.request!==id||!receipt.value)throw new Error('Stale Tools response.');
    return receipt.value;
  }

  function acceptPage(reply,expectedScope,expectedQuery){
    const next=reply?.page;
    if(reply?.kind!=='page'||!Number.isInteger(next?.generation)||next.generation<=0||
       next.generation>0xffffffff||next.scope!==(expectedScope?.task??null)||next.query!==expectedQuery||
       !Array.isArray(next.hits)||next.hits.length>30||!(next.next_before===null||cursor(next.next_before))||
       !bounded(next.coverage,512)||!next.hits.every(hit=>validAnchor(hit?.anchor)&&
         (!expectedScope||hit.anchor.conversation===expectedScope.task)&&bounded(hit.title,8192)&&
         bounded(hit.source,128)&&bounded(hit.snippet,1024)&&typeof hit.truncated==='boolean')){
      throw new Error('Invalid Workspace evidence page.');
    }
    return next;
  }

  function display(next){
    page=next;clearDocument();
    coverage.textContent=next.coverage;
    results.replaceChildren(...next.hits.map(hit=>{
      const entry=document.createElement('li'),button=document.createElement('button');
      const snippet=document.createElement('p');
      button.type='button';button.textContent=`${hit.title||'Untitled conversation'} · ${hit.source}`;
      button.dataset.eventId=hit.anchor.event_id;
      button.addEventListener('click',()=>work(marker=>openHit(marker,next.generation,hit.anchor)));
      snippet.textContent=hit.snippet;entry.append(button,snippet);
      return entry;
    }));
    status.textContent=next.hits.length?'':'No matching indexed records.';
  }

  async function search(marker){
    const selectedScope=scope(),text=query.value.trim();
    status.textContent='Searching Workspace evidence…';
    const reply=await call({operation:'search',query:text,scope:selectedScope,include_archived:archived.checked});
    if(marker!==intent)return;
    const next=acceptPage(reply,selectedScope,text);
    if(next.generation<=generation)throw new Error('An older evidence search was returned.');
    generation=next.generation;
    display(next);
  }

  async function older(marker){
    const current=page,selectedScope=scope();
    if(!current?.next_before)return;
    const reply=await call({operation:'more',generation:current.generation,before:current.next_before,scope:selectedScope});
    if(marker!==intent)return;
    const next=acceptPage(reply,selectedScope,current.query);
    if(next.generation!==current.generation||
       (next.next_before!==null&&BigInt(next.next_before)>=BigInt(current.next_before))){
      throw new Error('The Workspace evidence cursor did not advance.');
    }
    display(next);
  }

  async function openHit(marker,searchGeneration,anchor){
    if(page?.generation!==searchGeneration||!page.hits.some(hit=>sameAnchor(anchor,hit.anchor)))return;
    clearDocument();
    const reply=await call({operation:'open',generation:searchGeneration,anchor,scope:scope()});
    if(marker!==intent)return;
    const evidence=reply?.document;
    if(reply?.kind!=='document'||reply.generation!==searchGeneration||!sameAnchor(anchor,evidence?.anchor)||
       !bounded(evidence.source,128)||!bounded(evidence.text,262144)||typeof evidence.truncated!=='boolean'){
      throw new Error('Workspace returned a different evidence revision.');
    }
    evidenceSource.textContent=evidence.source;
    identity.textContent=`Record ${anchor.event_id} · Revision ${anchor.revision}`;
    identity.dataset.conversation=anchor.conversation;
    identity.dataset.eventId=anchor.event_id;
    identity.dataset.revision=anchor.revision;
    body.textContent=evidence.text;
    panel.hidden=false;
    status.textContent=evidence.truncated?'Showing a saved excerpt.':'Evidence opened.';
  }

  root.hidden=false;
  form.addEventListener('submit',event=>{
    event.preventDefault();
    if(!submit.disabled){retire();void work(search);}
  });
  function edit(){retire();controls();}
  query.addEventListener('input',edit);
  query.addEventListener('change',edit);
  archived.addEventListener('input',edit);
  scoped.addEventListener('change',controls);
  more.addEventListener('click',()=>work(older));
  controls();
  return {controls};
}
