import {installWorkspaceContext} from './workspace-context.js';
import {installWorkspaceMissions} from './workspace-missions.js';
import {installWorkspaceTools} from './workspace-tools.js';
import {installWorkspaceQueue} from './workspace-queue.js';

const uuid=value=>typeof value==='string'&&/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(value);
const decimal=value=>typeof value==='string'&&/^[0-9]{1,20}$/.test(value);
const taskId=value=>typeof value==='string'&&value.startsWith('jterm:task:')&&value.length<=16384;
const bounded=(value,limit)=>typeof value==='string'&&value.length<=limit;
const nullable=(value,limit)=>value===null||bounded(value,limit);
const same=(a,b)=>a?.task===b?.task&&a?.incarnation===b?.incarnation&&a?.generation===b?.generation;

export function validateNativeTask(task,previous=null){
  const identity=task?.identity;
  const verification=task?.verification;
  if(!taskId(identity?.task)||!uuid(identity.incarnation)||!Number.isInteger(identity.generation)||
     identity.generation<=0||identity.generation>0xffffffff||!decimal(task.sequence)||
     !bounded(task.title,512)||!bounded(task.status,128)||!bounded(task.worktree,8192)||
     !bounded(task.branch,1024)||!nullable(task.detail,65536)||!bounded(task.output,262144)||
     !nullable(task.draft_goal,16384)||
     !['truncated','controlled','can_stop','can_finish','can_inspect'].every(key=>typeof task[key]==='boolean')||
     !Array.isArray(task.approvals)||task.approvals.length>64||
     !nullable(verification?.recipe,65536)||!nullable(verification.shell,4096)||!bounded(verification.status,128)||
     !bounded(verification.output,65536)||typeof verification.truncated!=='boolean'||
     typeof verification.can_start!=='boolean'||
     (verification.can_start&&(!task.controlled||!verification.recipe||!verification.shell))){
    throw new Error('Invalid native task snapshot.');
  }
  if(previous&&(!same(identity,previous.identity)||BigInt(task.sequence)<BigInt(previous.sequence))){
    throw new Error('Stale or foreign native task snapshot.');
  }
  const ids=new Set();
  for(const approval of task.approvals){
    if(!uuid(approval?.id)||ids.has(approval.id)||!['Command','FileChange'].includes(approval.kind)||
       !nullable(approval.command,32768)||!nullable(approval.cwd,32768)||!nullable(approval.reason,32768)||
       !Array.isArray(approval.file_changes)||approval.file_changes.length>128||
       approval.file_changes.some(file=>!bounded(file?.path,32768)||!bounded(file.kind,256)||
           !bounded(file.diff,262144)||!nullable(file.move_path,32768))){
      throw new Error('Invalid native approval evidence.');
    }
    ids.add(approval.id);
  }
  return task;
}

function operationId(){
  const bytes=globalThis.crypto.getRandomValues(new Uint8Array(16));
  bytes[6]=(bytes[6]&15)|64;bytes[8]=(bytes[8]&63)|128;
  const hex=Array.from(bytes,byte=>byte.toString(16).padStart(2,'0')).join('');
  return [hex.slice(0,8),hex.slice(8,12),hex.slice(12,16),hex.slice(16,20),hex.slice(20)].join('-');
}

export function installNativeTasks(document,invoke){
  if(!invoke)return;
  const byId=id=>document.getElementById(id);
  const root=byId('workspace-native'),form=byId('native-create'),status=byId('native-status');
  const panel=byId('native-view'),start=byId('native-start'),select=byId('native-select');
  const saveDraft=byId('native-save-draft'),draftGoal=byId('native-draft-goal');
  const refresh=byId('native-refresh'),more=byId('native-more'),open=byId('native-open'),detach=byId('native-detach');
  const repository=byId('native-repository'),title=byId('native-title'),goal=byId('native-goal'),consent=byId('native-consent');
  const stop=byId('native-stop'),finish=byId('native-finish'),inspect=byId('native-inspect');
  const approvals=byId('native-approvals'),resultPanel=byId('native-result');
  const recipe=byId('native-verification-recipe'),verification=byId('native-verification');
  const verify=byId('native-verify'),accept=byId('native-accept-verified');
  const acceptReviewed=byId('native-accept-reviewed');
  let request=0,active=null,busy=false,cursor=null,catalogLoaded=false,polling=null;
  const missions=installWorkspaceMissions(document,{invoke,source:()=>active?.task});
  const tools=installWorkspaceTools(document,{invoke,source:()=>active?.task});
  const queue=installWorkspaceQueue(document,{
    invoke,source:()=>active?.task,
    launch(identity,revision){
      if(busy||!same(active?.task.identity,identity))throw new Error('The selected saved task changed.');
      return work(()=>decide({kind:'launch_draft',queue_revision:revision,consent:true}),error=>{throw error;});
    },
  });
  const context=installWorkspaceContext(document,{
    owner:()=>active?.task.identity,
    sameIdentity:same,call,work,
    canDraft:()=>!consent.checked&&![repository,title,goal,recipe].some(field=>field.value.length),
    openDraft(draft){
      clearView(active);
      repository.value=draft.repository;
      title.value=new TextEncoder().encode(draft.title).length<=160?draft.title:'';
      goal.value=draft.body;
      status.textContent='New task draft prepared. Review before starting.';
      requestAnimationFrame(()=>title.focus());
    },
  });

  async function call(action){
    const id=++request;
    if(id>0xffffffff)throw new Error('Native task request counter exhausted.');
    const receipt=await invoke('workspace_native',{request:id,action});
    if(receipt?.request!==id)throw new Error('Stale native task receipt.');
    return receipt.value;
  }

  function controls(){
    refresh.disabled=busy||!!active;
    more.disabled=refresh.disabled||!cursor;
    select.disabled=refresh.disabled||!select.options.length;
    open.disabled=select.disabled||!select.value;
    detach.disabled=busy||!active;
    start.disabled=busy||!!active||!consent.checked;
    saveDraft.disabled=busy||!!active;
    for(const field of [repository,title,goal,recipe,consent])field.disabled=busy||!!active;
    stop.disabled=busy||!active?.task.can_stop;
    finish.disabled=busy||!active?.task.can_finish;
    inspect.disabled=busy||!active?.task.can_inspect;
    verify.disabled=busy||!active?.task.verification.can_start;
    const canAccept=active?.task.controlled&&active.review?.sequence===active.task.sequence;
    accept.disabled=busy||!canAccept||active.review.kind!=='accept_verified';
    acceptReviewed.disabled=busy||!canAccept||active.review.kind!=='accept_manual';
    for(const button of approvals.querySelectorAll('button')){
      button.disabled=busy||!active?.task.controlled;
    }
    context.controls(busy);
    missions.controls();
    tools.controls();
    queue.controls(busy);
  }

  function work(task,onError=error=>{status.textContent=String(error?.message??error);}){
    if(busy)return;
    busy=true;controls();
    return Promise.resolve(polling).then(task).catch(onError)
      .finally(()=>{busy=false;controls();});
  }

  function node(tag,text){
    const value=document.createElement(tag);
    if(text!==undefined)value.textContent=text;
    return value;
  }

  function text(id,value){
    const element=byId(id);
    if(element.textContent!==value)element.textContent=value;
  }

  async function list(before=null){
    const reply=await call({operation:'list',before});
    if(reply?.kind!=='catalog'||!Array.isArray(reply.rows)||reply.rows.length>24||
       !(reply.next_before===null||decimal(reply.next_before))||
       reply.rows.some(row=>!taskId(row?.id)||!bounded(row.title,512))){
      throw new Error('Invalid native catalog receipt.');
    }
    cursor=reply.next_before;catalogLoaded=true;
    select.replaceChildren(...reply.rows.map(row=>{
      const option=node('option',row.title);option.value=row.id;return option;
    }));
    status.textContent=reply.rows.length?'':cursor?'Use Next page to continue.':'No native tasks on this page.';
  }

  function render(view,task){
    const previous=view.task;
    view.task=task;
    if(previous?.sequence===task.sequence&&previous.controlled===task.controlled)return;
    if(previous?.sequence!==task.sequence){
      view.review=null;
      resultPanel.hidden=true;
    }
    text('native-task-title',task.title);
    text('native-worktree',`${task.branch} · ${task.worktree}`);
    text('native-lifecycle',task.status);
    text('native-detail',task.detail??'');
    draftGoal.hidden=task.draft_goal===null;
    text('native-draft-goal',task.draft_goal??'');
    text('native-output',task.output+(task.truncated?'\n[Earlier provider output was trimmed.]':''));
    const check=task.verification;
    verification.hidden=check.recipe===null;
    text('native-verification-shell',check.shell?`Shell ${check.shell}`:'');
    text('native-verification-command',check.recipe??'');
    text('native-verification-status',check.status);
    text('native-verification-output',check.output+(check.truncated?'\n[Earlier verification output was trimmed.]':''));
    stop.textContent=check.status==='Running'?'Stop verification':'Stop task';
    const sameApprovals=JSON.stringify(previous?.approvals)===JSON.stringify(task.approvals);
    if(!sameApprovals){
      approvals.replaceChildren(...task.approvals.map(approval=>{
        const card=node('article');
        card.append(node('h3',approval.kind==='Command'?'Command approval request':'File change approval request'));
        if(approval.command)card.append(node('pre',approval.command));
        if(approval.cwd)card.append(node('p',approval.cwd));
        if(approval.reason)card.append(node('p',approval.reason));
        for(const file of approval.file_changes)card.append(node('pre',`${file.path}\n${file.diff}`));
        const decline=node('button','Decline request');decline.type='button';
        decline.addEventListener('click',()=>work(()=>decide({kind:'decline',approval:approval.id})));
        card.append(decline);
        return card;
      }));
    }
  }

  async function choose(action){
    status.textContent=action.operation==='start'?'Preparing a managed worktree and native task…':
      action.operation==='save_draft'?'Saving the authored task draft…':'Opening the native task…';
    const reply=await call(action);
    if(reply?.kind!=='task')throw new Error('Missing native task admission receipt.');
    const task=validateNativeTask(reply.task);
    if(action.task&&task.identity.task!==action.task)throw new Error('Native task selection changed.');
    const view={task:null,timer:null,review:null};
    active=view;panel.hidden=false;resultPanel.hidden=true;
    view.timer=setInterval(()=>void observe(view),250);
    render(view,task);
    status.textContent=task.draft_goal!==null?'Task draft saved. Open Task queue to review its readiness.':
      task.controlled?'Native task view attached.':'Viewing native task history and current evidence.';
    if(action.operation==='start'||action.operation==='save_draft')form.reset();
  }

  function observe(view){
    if(busy||polling||active!==view)return;
    polling=call({operation:'observe',identity:view.task.identity}).then(reply=>{
      if(active!==view)return;
      if(reply?.kind!=='task')throw new Error('Missing native task progress receipt.');
      render(view,validateNativeTask(reply.task,view.task));
      controls();
    }).catch(error=>{
      if(active===view&&!busy)status.textContent=String(error?.message??error);
    }).finally(()=>{polling=null;});
  }

  async function decide(decision){
    const view=active,task=view.task;
    if(['verify','accept_verified','accept_manual'].includes(decision.kind))view.review=null;
    const reply=await call({
      operation:'decide',identity:task.identity,sequence:task.sequence,operation_id:operationId(),decision,
    });
    if(active!==view)return;
    if(reply?.kind!=='task')throw new Error('Missing native task operation receipt.');
    render(view,validateNativeTask(reply.task,task));
    status.textContent=decision.kind==='accept_verified'?'Verified work accepted by the Workspace host.':
      decision.kind==='accept_manual'?'Reviewed work accepted by the Workspace host.':
      decision.kind==='launch_draft'?'Queued task launch recorded by the Workspace host.':
      decision.kind==='verify'?'Verification request received by the Workspace host.':
      'Operation received by the Workspace host.';
  }

  async function inspectResult(){
    const view=active;
    view.review=null;
    const reply=await call({operation:'result',identity:view.task.identity});
    if(active!==view)return;
    const result=reply?.result;
    if(reply?.kind!=='result'||!same(result?.identity,view.task.identity)||!decimal(result.sequence)||
       result.sequence!==view.task.sequence||!bounded(result.status,262144)||
       !bounded(result.diff,1048576)||typeof result.truncated!=='boolean'||
       !bounded(result.validation,256)||typeof result.can_accept!=='boolean'||
       typeof result.can_accept_review!=='boolean'||(result.can_accept&&result.can_accept_review)||
       !nullable(result.detail,65536)){
      throw new Error('Invalid native result inspection receipt.');
    }
    text('native-validation',result.can_accept_review?'Human review required':result.validation);
    text('native-diff',[result.detail,result.status,result.diff,
      result.truncated?'[Worktree evidence was trimmed.]':''].filter(Boolean).join('\n')||
      'No worktree changes since the recorded baseline.');
    resultPanel.hidden=false;
    accept.hidden=!result.can_accept;
    acceptReviewed.hidden=!result.can_accept_review;
    if(result.can_accept||result.can_accept_review){
      view.review={sequence:result.sequence,kind:result.can_accept?'accept_verified':'accept_manual'};
    }
    status.textContent='Worktree inspected.';
  }

  async function release(){
    const view=active;
    const reply=await call({operation:'detach',identity:view.task.identity});
    if(reply?.kind!=='ack')throw new Error('Missing native task detach acknowledgement.');
    clearView(view);
    status.textContent='Native task view detached.';
  }

  function clearView(view){
    clearInterval(view.timer);active=null;panel.hidden=true;resultPanel.hidden=true;
    approvals.replaceChildren();text('native-output','');
    text('native-verification-command','');text('native-verification-output','');
    context.reset();
  }

  root.hidden=false;
  root.addEventListener('toggle',()=>{if(root.open&&!catalogLoaded&&!active)void work(()=>list());});
  refresh.addEventListener('click',()=>work(()=>list()));
  more.addEventListener('click',()=>work(()=>list(cursor)));
  select.addEventListener('change',controls);
  open.addEventListener('click',()=>work(()=>choose({operation:'open',task:select.value})));
  detach.addEventListener('click',()=>work(release));
  consent.addEventListener('change',controls);
  form.addEventListener('submit',event=>{
    event.preventDefault();
    if(!start.disabled)void work(()=>choose({
      operation:'start',repository:repository.value,title:title.value,goal:goal.value,consent:consent.checked,
      verification:recipe.value.trim()?recipe.value:null,
    }));
  });
  saveDraft.addEventListener('click',()=>{
    if(saveDraft.disabled||![repository,title,goal].every(field=>field.reportValidity()))return;
    void work(()=>choose({operation:'save_draft',repository:repository.value,title:title.value,
      goal:goal.value,verification:recipe.value.trim()?recipe.value:null}));
  });
  stop.addEventListener('click',()=>work(()=>decide({kind:'stop'})));
  finish.addEventListener('click',()=>work(()=>decide({kind:'finish'})));
  inspect.addEventListener('click',()=>work(inspectResult));
  verify.addEventListener('click',()=>work(()=>decide({kind:'verify'})));
  accept.addEventListener('click',()=>work(()=>decide({kind:'accept_verified'})));
  acceptReviewed.addEventListener('click',()=>work(()=>decide({kind:'accept_manual'})));
  controls();
}

if(typeof document!=='undefined')installNativeTasks(document,globalThis.__TAURI__?.core?.invoke);
