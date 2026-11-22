const bounded=(value,limit)=>typeof value==='string'&&value.length<=limit;
const revision=value=>typeof value==='string'&&/^[1-9][0-9]{0,19}$/.test(value);
const sameView=(left,right)=>left?.id===right?.id&&left?.generation===right?.generation;
const states={
  draft:'Draft',in_progress:'Delivery in progress',partially_integrated:'Partially integrated',
  needs_attention:'Needs your attention',completed:'Mission completed',
};
const checks={passed:'Passed',failed:'Failed',blocked:'Blocked',not_run:'Not run',stale:'Stale'};

export function installWorkspaceMissions(document,{invoke,source}){
  const byId=id=>document.getElementById('mission-'+id);
  const root=document.getElementById('workspace-mission'),panel=byId('view'),status=byId('status');
  const select=byId('select'),open=byId('open'),refresh=byId('refresh'),create=byId('new');
  const createForm=byId('create'),editForm=byId('edit');
  const title=byId('title'),goal=byId('goal'),repository=byId('repository'),target=byId('target');
  const editTitle=byId('edit-title'),editGoal=byId('edit-goal'),save=byId('save');
  const inspect=byId('inspect'),freeze=byId('freeze'),reportPanel=byId('report');
  let sequence=0,busy=false,loaded=false,selected=null;

  function dirty(){
    return !!selected&&(editTitle.value!==selected.title||editGoal.value!==selected.goal);
  }
  function controls(){
    const active=source(),editing=dirty();
    byId('source').textContent=active?`From native task “${active.title}”`:'Open a native task to link its worktree.';
    refresh.disabled=busy;
    select.disabled=busy||!select.options.length;
    open.disabled=busy||!select.value;
    open.textContent=editing?'Discard edits and open selected':'Open Mission';
    create.disabled=busy||!active||editing;
    for(const input of [title,goal,repository,target,editTitle,editGoal])input.disabled=busy;
    save.disabled=busy||!editing;
    inspect.disabled=busy||!selected||editing;
    freeze.disabled=busy||!selected?.can_freeze||editing;
  }

  function work(action){
    if(busy)return;
    busy=true;controls();
    return action().catch(error=>{status.textContent=String(error?.message??error);})
      .finally(()=>{busy=false;controls();});
  }

  function node(tag,text){
    const element=document.createElement(tag);
    if(text!==undefined)element.textContent=text;
    return element;
  }

  function catalog(rows,preferred){
    if(!Array.isArray(rows)||rows.length>64||rows.some(row=>!bounded(row?.id,128)||!bounded(row.title,256))){
      throw new Error('Invalid Mission catalog receipt.');
    }
    const current=preferred??select.value;
    select.replaceChildren(...rows.map(row=>{
      const option=node('option',row.title);option.value=row.id;return option;
    }));
    if(rows.some(row=>row.id===current))select.value=current;
    loaded=true;
  }

  async function dispatch(choice){
    const request=++sequence;
    if(request>0xffffffff)throw new Error('Mission request counter exhausted.');
    const receipt=await invoke('workspace_mission',{request,choice});
    if(receipt?.request!==request||!receipt.value)throw new Error('Stale Mission response.');
    return receipt.value;
  }

  function validView(view){
    const identity=view?.identity;
    if(!bounded(identity?.id,128)||!identity.id||!revision(identity.revision)||
       !Number.isInteger(identity.generation)||identity.generation<=0||identity.generation>0xffffffff||
       !bounded(view.title,256)||!bounded(view.goal,16384)||!bounded(view.environment,512)||
       !Array.isArray(view.repositories)||view.repositories.length>16||
       !['current_inspection','can_freeze'].every(key=>typeof view[key]==='boolean')||
       (view.can_freeze&&!view.current_inspection)){
      throw new Error('Invalid Mission plan receipt.');
    }
    if(view.report!==null&&(
      view.report?.id!==identity.id||!Object.hasOwn(states,view.report.state)||
      !Array.isArray(view.report.repositories)||view.report.repositories.length>16||
      !Array.isArray(view.report.errors))){
      throw new Error('Invalid Mission evidence receipt.');
    }
    return view;
  }

  function renderPlan(view){
    const entries=view.repositories.map(repo=>{
      const card=node('article');
      card.append(node('h3',repo.identity.full_name),node('p',repo.path),
        node('p',`Target ${repo.target_ref}`),node('p',`Baseline ${repo.baseline||'pending'}`),
        node('p',`Candidate ${repo.candidate||'not frozen'}`));
      for(const task of repo.tasks){
        card.append(node('p',`Linked task ${task.task_id} · Run ${task.run_id}`));
      }
      return card;
    });
    byId('repositories').replaceChildren(...entries);
  }

  function renderReport(view){
    const report=view?.report;
    reportPanel.hidden=!report;
    const output=byId('evidence');
    if(!report){output.replaceChildren();return;}
    byId('report-title').textContent=states[report.state];
    byId('evidence-state').textContent=view.current_inspection?
      `Current inspection · ${view.environment}`:'Saved observation · inspect again to verify current evidence';
    output.replaceChildren(...report.errors.map(error=>node('p',error)),...report.repositories.map(repo=>{
      const card=node('article');
      card.append(node('h4',repo.identity.full_name));
      if(repo.observed){
        card.append(node('p',`Observed HEAD ${repo.observed.head}`),
          node('p',repo.observed.dirty?'Worktree has uncommitted changes':'Worktree is clean'));
      }
      card.append(node('p',`Commit ${repo.committed?'verified':'pending'} · Push ${repo.pushed?'verified':'unverified'} · Integration ${repo.integrated?'verified':'unverified'}`));
      for(const check of repo.checks){
        const text=[`${check.check_name} · ${checks[check.status]??'Unknown'} · ${check.environment}`,
          check.receipt_id,check.detail].filter(Boolean).join(' · ');
        const result=node('p',text);
        result.dataset.check=check.status;
        card.append(result);
      }
      for(const error of repo.errors)card.append(node('p',error));
      return card;
    }));
  }

  function display(value,message){
    const view=validView(value.view);
    catalog(value.rows,view.identity.id);
    selected=view;
    editTitle.value=view.title;
    editGoal.value=view.goal;
    byId('selected-title').textContent=view.title;
    renderPlan(view);renderReport(view);panel.hidden=false;
    status.textContent=`${message} · revision ${view.identity.revision}.`;
  }

  async function refreshAll(){
    if(selected){selected.can_freeze=false;renderReport(null);}
    const value=await dispatch({operation:'list'});
    catalog(value.rows);
    status.textContent=value.rows.length?'Mission catalog refreshed.':'No saved Missions.';
  }

  async function load(){
    const id=select.value,prior=selected?.identity.generation??0;
    const value=await dispatch({operation:'open',id}),view=validView(value.view);
    if(view.identity.id!==id||view.identity.generation<=prior)throw new Error('Mission selection changed.');
    display(value,'Mission opened');
  }

  async function createMission(){
    const task=source(),prior=selected?.identity.generation??0;
    if(!task||dirty())return;
    const value=await dispatch({operation:'create',source:task.identity,title:title.value,goal:goal.value,
      repository:repository.value,target_ref:target.value});
    const view=validView(value.view);
    if(view.identity.generation<=prior||view.identity.revision!=='1'||
       !view.repositories.some(repo=>repo.tasks.some(link=>
         link.conversation===task.identity.task&&link.run_id===task.identity.incarnation))){
      throw new Error('Mission did not link the selected native task.');
    }
    display(value,'Mission saved');
    createForm.reset();
  }

  async function selectedAction(operation,extra,message,mutated){
    const previous=selected.identity;
    selected.can_freeze=false;
    renderReport(null);
    const value=await dispatch({operation,identity:previous,...extra}),view=validView(value.view);
    if(!sameView(previous,view.identity)||
       BigInt(view.identity.revision)!==BigInt(previous.revision)+(mutated?1n:0n)){
      throw new Error('Mission revision changed. Reopen the saved plan.');
    }
    display(value,message);
  }

  root.hidden=false;
  root.addEventListener('toggle',()=>{if(root.open&&!loaded)void work(refreshAll);});
  refresh.addEventListener('click',()=>work(refreshAll));
  open.addEventListener('click',()=>work(load));
  select.addEventListener('change',controls);
  createForm.addEventListener('submit',event=>{event.preventDefault();if(!create.disabled)void work(createMission);});
  for(const input of [editTitle,editGoal])input.addEventListener('input',controls);
  editForm.addEventListener('submit',event=>{
    event.preventDefault();
    if(!save.disabled)void work(()=>selectedAction('save',{title:editTitle.value,goal:editGoal.value},'Mission saved',true));
  });
  inspect.addEventListener('click',()=>work(()=>selectedAction('inspect',{},'Delivery evidence inspected',false)));
  freeze.addEventListener('click',()=>work(()=>selectedAction('freeze',{},'Candidates frozen',true)));
  controls();
  return {controls};
}
