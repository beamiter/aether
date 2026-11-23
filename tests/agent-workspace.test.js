import test from 'node:test';
import assert from 'node:assert/strict';
import {installWorkspace,validateWorkspaceAttachment,validateWorkspacePage} from '../web/agent-workspace.js';

const native='11111111-1111-4111-8111-111111111111';
const profile=path=>'jterm:cli:v1:'+JSON.stringify(['Codex','fixture-host',path,native]);
const first=profile('/home/one'),second=profile('/home/two');
const row=id=>({id,title:'Conversation',provider:'Codex',directory:'/project',status:'Ready to resume',reopenable:true});

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
      fire(kind){if(kind!=='click'||!this.disabled)return this.events.get(kind)?.();}
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
