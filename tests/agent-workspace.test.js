import test from 'node:test';
import assert from 'node:assert/strict';
import {validateWorkspaceAttachment,validateWorkspacePage} from '../web/agent-workspace.js';

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
