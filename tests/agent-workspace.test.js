import test from 'node:test';
import assert from 'node:assert/strict';
import {installWorkspace,validateWorkspaceAttachment,validateWorkspacePage} from '../web/agent-workspace.js';

const native='11111111-1111-4111-8111-111111111111';
const profile=path=>'jterm:cli:v1:'+JSON.stringify(['Codex','fixture-host',path,native]);
const first=profile('/home/one'),second=profile('/home/two');
const row=id=>({id,title:'Conversation',provider:'Codex',directory:'/project',status:'Ready to resume',reopenable:true,removable:true});

test('the Workspace catalog keeps equal native IDs under separate canonical sources',()=>{
  const page={generation:1,rows:[row(first),row(second)],next_before:'19'};
  assert.equal(validateWorkspacePage(page),page);
  assert.throws(()=>validateWorkspacePage({...page,rows:[row(first),row(first)]}),/Invalid Workspace conversation/);
  assert.throws(()=>validateWorkspacePage({...page,rows:[row(native)]}),/Invalid Workspace conversation/);
  assert.throws(()=>validateWorkspacePage({...page,next_before:19}),/Invalid Workspace catalog/);
});

test('an attachment is admitted only for the requested source and a complete view identity',()=>{
  const identity={conversation:first,owner:{session:'a'.repeat(32),epoch:3,generation:3}};
  assert.equal(validateWorkspaceAttachment({identity},first),identity);
  assert.throws(()=>validateWorkspaceAttachment({identity},second),/Invalid Workspace terminal attachment/);
  assert.throws(()=>validateWorkspaceAttachment({identity:{...identity,owner:{...identity.owner,generation:0}}},first));
  assert.throws(()=>validateWorkspaceAttachment({identity:{...identity,owner:{...identity.owner,session:'fallback'}}},first));
});

function catalogHarness(handle){
  const fields=new Map(),listeners=new Map(),windowListeners=new Map();
  let document;
  const body={},focusLog=[];
  function element(){
    return {disabled:false,value:'',textContent:'',children:[],events:new Map(),style:{},
      focus(){
        if(this.disabled)return;
        document.activeElement=this;focusLog.push(this);
        document.dispatch('focusin',{target:this});
      },
      getContext(){return {setTransform(){},clearRect(){},fillRect(){},save(){},beginPath(){},
        rect(){},clip(){},fillText(){},restore(){}};},
      addEventListener(kind,callback){this.events.set(kind,callback);},
      replaceChildren(...children){this.children=children;this.value=children[0]?.value??'';},
      fire(kind,event){if(kind!=='click'||!this.disabled)return this.events.get(kind)?.(event);}
    };
  }
  const get=name=>{
    if(!fields.has(name))fields.set(name,element());
    return fields.get(name);
  };
  const eventTarget=registered=>({
    addEventListener(kind,handler,capture=false){
      if(!registered.has(kind))registered.set(kind,new Map());
      registered.get(kind).set(handler,capture);
    },
    removeEventListener(kind,handler,capture=false){
      if(registered.get(kind)?.get(handler)===capture)registered.get(kind).delete(handler);
    },
    dispatch(kind,event={}){
      for(const handler of registered.get(kind)?.keys()??[])handler(event);
    },
  });
  document={getElementById:get,createElement:element,body,activeElement:body,foreground:true,hidden:false,
    defaultView:eventTarget(windowListeners),
    hasFocus(){return this.foreground;},
    ...eventTarget(listeners)};
  get('agent-workspace').open=true;
  const calls=[];
  installWorkspace(document,async(command,args)=>{
    calls.push([command,args]);
    return {request:args.request,value:await handle(command,args)};
  });
  const button=id=>get('workspace-'+id);
  const pick=id=>{button('conversation').value=id;button('conversation').fire('change');};
  const act=async id=>{button(id).fire('click');await new Promise(setImmediate);};
  return {button,calls,pick,act,document,get,focusLog,listeners,windowListeners,selection:()=>button('conversation').value};
}

test('Workspace removal requires confirmation for the selected profile and reloads after its exact receipt',async()=>{
  let generation=1,remaining=[row(first),row(second)];
  const h=catalogHarness((command,args)=>{
    if(command==='workspace_remove_cli'){
      assert.deepEqual(args,{request:2,conversation:second,catalogGeneration:1});
      remaining=[row(first)];generation++;
      return second;
    }
    assert.equal(command,'workspace_catalog');
    return {generation,rows:remaining,next_before:null};
  });
  await h.act('refresh');
  h.pick(second);
  await h.act('remove');
  assert.match(h.button('remove-selection').textContent,/Profile 2 · Entry 2/);
  assert.equal(h.calls.length,1);
  h.pick(first);
  assert.equal(h.button('remove-review').hidden,true);
  await h.act('remove-confirm');
  assert.equal(h.calls.length,1);
  h.pick(second);
  await h.act('remove');
  await h.act('remove-cancel');
  assert.equal(h.calls.length,1);
  await h.act('remove');
  await h.act('remove-confirm');
  assert.deepEqual(h.button('conversation').children.map(option=>option.value),[first]);
  assert.match(h.button('status').textContent,/Conversation removed/);
  assert.deepEqual(h.calls.map(([command])=>command),['workspace_catalog','workspace_remove_cli','workspace_catalog']);
});

test('Workspace retires the old catalog if removal has no matching receipt',async()=>{
  const h=catalogHarness(command=>command==='workspace_catalog'
    ?{generation:1,rows:[row(first),row(second)],next_before:null}:first);
  await h.act('refresh');
  h.pick(second);
  await h.act('remove');
  await h.act('remove-confirm');
  assert.equal(h.selection(),'');
  assert.equal(h.button('remove').disabled,true);
  assert.match(h.button('status').textContent,/Invalid Workspace removal receipt.*Refresh/);
  assert.deepEqual(h.calls.map(([command])=>command),['workspace_catalog','workspace_remove_cli']);
});

test('Workspace Previous restores the exact profile without replaying the host cursor',async()=>{
  let expected=null;
  const third=profile('/home/other');
  const h=catalogHarness((command,args)=>{
    if(command==='workspace_attach')throw new Error('fixture blocks the lease');
    assert.equal(command,'workspace_catalog');
    assert.deepEqual(args.cursor,expected);
    if(expected===null){
      expected={generation:1,before:'203'};
      return {generation:1,rows:[row(first),row(second)],next_before:'203'};
    }
    return {generation:1,rows:[row(third)],next_before:null};
  });
  await h.act('refresh');
  h.pick(second);
  await h.act('more');
  assert.equal(h.selection(),third);
  assert.equal(h.button('more').disabled,true);
  await h.act('previous');
  assert.equal(h.selection(),second);
  assert.equal(h.button('previous').disabled,true);
  assert.equal(h.calls.length,2);
  await h.act('more');
  assert.equal(h.selection(),third);
  await h.act('previous');
  assert.equal(h.selection(),second);
  assert.equal(h.calls.length,2,'cached navigation must not request or launch anything');
  await h.act('attach');
  assert.deepEqual(h.calls.at(-1),['workspace_attach',{request:3,conversation:second,catalogGeneration:1}]);
});

test('Workspace caches only four recent pages and refresh restarts navigation',async()=>{
  let generation=0,page=0;
  const h=catalogHarness((command,{cursor})=>{
    assert.equal(command,'workspace_catalog');
    if(cursor===null){generation++;page=0;}
    else{assert.deepEqual(cursor,{generation,before:String(200-page)});page++;}
    return {generation,rows:[row(profile('/home/p'+page))],next_before:String(200-page)};
  });
  await h.act('refresh');
  for(let i=0;i<5;i++)await h.act('more');
  assert.equal(h.selection(),profile('/home/p5'));
  assert.equal(h.calls.length,6);
  for(let i=0;i<3;i++)await h.act('previous');
  assert.equal(h.selection(),profile('/home/p2'));
  assert.match(h.button('status').textContent,/Earlier pages were released/);
  assert.equal(h.button('previous').disabled,true);
  await h.act('previous');
  assert.equal(h.calls.length,6);
  await h.act('refresh');
  assert.equal(h.selection(),profile('/home/p0'));
  assert.equal(h.button('previous').disabled,true);
  assert.equal(h.calls.length,7);
  assert.equal(h.calls.at(-1)[1].cursor,null);
});

test('Workspace traverses empty filtered pages and invalidates a lost or non-advancing frontier',async()=>{
  for(const failure of ['lost','changed','non-advancing']){
    let page=0;
    const h=catalogHarness((command,{cursor})=>{
      assert.equal(command,'workspace_catalog');
      if(++page===1){
        assert.equal(cursor,null);
        return {generation:1,rows:[row(first)],next_before:'203'};
      }
      if(page===2){
        assert.deepEqual(cursor,{generation:1,before:'203'});
        return {generation:1,rows:[],next_before:'112'};
      }
      if(page===3){
        assert.deepEqual(cursor,{generation:1,before:'112'});
        if(failure==='lost')throw new Error('response lost after cursor advanced');
        return {generation:failure==='changed'?2:1,rows:[row(second)],next_before:failure==='changed'?'100':'112'};
      }
      assert.equal(cursor,null);
      return {generation:2,rows:[row(second)],next_before:null};
    });
    await h.act('refresh');
    await h.act('more');
    assert.equal(h.button('attach').disabled,true);
    assert.equal(h.button('more').disabled,false);
    await h.act('previous');
    assert.equal(h.selection(),first);
    await h.act('more');
    assert.equal(h.calls.length,2);
    await h.act('more');
    assert.equal(h.selection(),'');
    for(const control of ['attach','conversation','previous','more'])assert.equal(h.button(control).disabled,true,control);
    await h.act('more');
    await h.act('attach');
    assert.equal(h.calls.length,3);
    assert.match(h.button('status').textContent,/Refresh the catalog/);
    await h.act('refresh');
    assert.equal(h.selection(),second);
    assert.equal(h.button('previous').disabled,true);
  }
});

test('Workspace attach cannot steal focus after the user moves away before first poll',async()=>{
  const owner={session:'a'.repeat(32),epoch:3,generation:3};
  const identity={conversation:first,owner};
  const frame={version:3,session:owner.session,epoch:owner.epoch,revision:1,layout:1,cols:1,
    rows:[[{text:'a',foreground:'#ffffff',background:'#000000',width:1,style:0}]],
    cursor_row:0,cursor_col:0,cursor_visible:false,cursor_shape:'block',pending_wrap:false};
  for(const mode of ['stay','moved','moved-then-body','button-blurred','started-elsewhere','collapsed','background','read-only',
    'blur-return','hidden-return','visible-event']){
    let finishPoll;
    const firstPoll=new Promise(resolve=>{finishPoll=resolve;});
    const h=catalogHarness(async(command,args)=>{
      if(command==='workspace_catalog')return {generation:1,rows:[row(first)],next_before:null};
      if(command==='workspace_attach')return {identity,frame};
      assert.equal(command,'workspace_terminal');
      assert.deepEqual(args.identity,identity);
      if(args.operation.operation==='poll'){await firstPoll;return {kind:'polled',
        update:{frame:null,writable:mode!=='read-only',status:'Fixture ready.'}};}
      return {kind:'receipt'};
    });
    await h.act('refresh');
    const attach=h.button('attach'),input=h.button('input');
    if(mode==='started-elsewhere')h.get('notebook-search').focus();
    else attach.focus();
    const result=attach.fire('click');
    await new Promise(setImmediate);
    try{
      assert.deepEqual(h.calls.slice(1).map(([command,args])=>[command,args.operation?.operation]),
        [['workspace_attach',undefined],['workspace_terminal','ack'],['workspace_terminal','poll']]);
      assert.equal(input.disabled,true,'writability waits for first confirmed update');
      if(mode==='moved'||mode==='moved-then-body')h.get('notebook-search').focus();
      if(mode==='moved-then-body'||mode==='button-blurred')h.document.activeElement=h.document.body;
      if(mode==='collapsed')h.get('agent-workspace').open=false;
      if(mode==='background')h.document.foreground=false;
      if(mode==='blur-return'){
        h.document.foreground=false;
        h.document.defaultView.dispatch('blur');
        h.document.foreground=true;
      }
      if(mode==='hidden-return'){
        h.document.hidden=true;
        h.document.dispatch('visibilitychange');
        h.document.hidden=false;
      }
      if(mode==='visible-event')h.document.dispatch('visibilitychange');
      const before=h.document.activeElement;
      finishPoll();
      await result;
      assert.equal(input.disabled,mode==='read-only');
      const followsAttach=mode==='stay'||mode==='button-blurred'||mode==='visible-event';
      assert.equal(h.document.activeElement,followsAttach?input:before,mode);
      assert.equal(h.focusLog.filter(element=>element===input).length,followsAttach?1:0,mode);
      assert.equal(h.listeners.get('focusin')?.size??0,0,'focus listener must retire');
      assert.equal(h.listeners.get('visibilitychange')?.size??0,0,'visibility listener must retire');
      assert.equal(h.windowListeners.get('blur')?.size??0,0,'window listener must retire');
    }finally{
      finishPoll();
      await result;
      await h.act('detach');
    }
  }
});

test('Workspace multiline paste needs confirmation and expires on detach',async()=>{
  const owner={session:'b'.repeat(32),epoch:4,generation:4};
  const identity={conversation:first,owner};
  const frame={version:3,session:owner.session,epoch:owner.epoch,revision:1,layout:1,cols:1,
    rows:[[{text:'a',foreground:'#ffffff',background:'#000000',width:1,style:0}]],
    cursor_row:0,cursor_col:0,cursor_visible:false,cursor_shape:'block',pending_wrap:false};
  const h=catalogHarness((command,args)=>{
    if(command==='workspace_catalog')return {generation:1,rows:[row(first)],next_before:null};
    if(command==='workspace_attach')return {identity,frame};
    if(args.operation.operation==='poll')return {kind:'polled',
      update:{frame:null,writable:true,status:'Fixture ready.'}};
    return {kind:'receipt'};
  });
  const sent=()=>h.calls.filter(([,args])=>args.operation?.operation==='paste');
  const offer=text=>{
    let prevented=false;
    h.button('input').fire('paste',{
      preventDefault(){prevented=true;},clipboardData:{getData:()=>text}
    });
    assert.equal(prevented,true);
  };
  await h.act('refresh');
  await h.act('attach');
  try{
    assert.equal(h.button('input').disabled,false);
    offer('first\r\nsecond');
    assert.equal(h.button('paste-review').hidden,false);
    assert.equal(sent().length,0);
    await h.act('paste-confirm');
    assert.deepEqual(sent().map(([,args])=>args.operation.text),['first\r\nsecond']);
    offer('queued\nreview');
    await h.act('detach');
    assert.equal(h.button('paste-review').hidden,true);
    await h.act('paste-confirm');
    assert.equal(sent().length,1);
  }finally{
    await h.act('detach');
  }
});

test('same-title Workspace choices distinguish source profiles without revealing their names',async()=>{
  const third='jterm:cli:v1:'+JSON.stringify(['Codex','fixture-host','/home/one','local-other-id']);
  const metadataOnly=profile('/home/private-metadata');
  const malformed='jterm:cli:v1:not-a-source-tuple';
  const unique=profile('/home/unique');
  const rows=[row(first),row(second),row(third),
    {...row(metadataOnly),reopenable:false,removable:false},{...row(malformed),reopenable:false,removable:false},
    {...row(unique),title:'Unique session'}];
  const h=catalogHarness((command,args)=>{
    if(command==='workspace_attach')throw new Error('fixture blocks the lease');
    assert.equal(command,'workspace_catalog');assert.equal(args.cursor,null);
    return {generation:1,rows,next_before:null};
  });
  await h.act('refresh');
  const labels=h.button('conversation').children.map(option=>option.textContent);
  assert.deepEqual(labels,[
    'Conversation · Profile 1 · Entry 1',
    'Conversation · Profile 2 · Entry 2',
    'Conversation · Profile 1 · Entry 3',
    'Conversation · Entry 4',
    'Conversation · Entry 5',
    'Unique session',
  ]);
  h.pick(second);
  const detail=h.button('selection').textContent;
  assert.match(detail,/Profile 2 · Entry 2 \(on this page\)$/);
  assert.doesNotMatch([detail,...labels].join('\n'),/\/home\/|fixture-host|11111111|local-other-id/);
  assert.equal(h.calls.length,1,'selecting a display label must not invoke or attach');
  await h.act('attach');
  assert.deepEqual(h.calls.at(-1),['workspace_attach',{request:2,conversation:second,catalogGeneration:1}]);
});
