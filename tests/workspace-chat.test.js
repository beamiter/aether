import test from 'node:test';
import assert from 'node:assert/strict';
import {validateChatJob,validateChatView} from '../web/workspace-chat.js';

const identity={conversation:'jterm:api:fixture',generation:3};
const job={epoch:'2',sequence:'3',state:'Running',user_text:'multi\nline',preview:'first',can_cancel:true};
const view={
  identity,title:'Fixture conversation',archived:false,background_supported:true,terminal_context:false,
  revision:'5',draft_revision:'4',draft:'newer',turns:[],has_older:false,job,
};

test('API drafts and jobs remain scoped to the exact conversation view',()=>{
  assert.equal(validateChatView(view,identity),view);
  assert.throws(()=>validateChatView(view,{...identity,generation:2}),/Invalid API conversation view/);
  assert.throws(()=>validateChatView(view,{...identity,conversation:'jterm:api:other'}),/Invalid API conversation view/);
  assert.throws(()=>validateChatView({...view,draft_revision:4}),/Invalid API conversation view/);
});

test('an old streaming snapshot cannot replace a later preview or a different execution',()=>{
  assert.equal(validateChatJob(job,job),job);
  assert.throws(()=>validateChatJob({...job,sequence:'2'},job),/Stale API request snapshot/);
  assert.throws(()=>validateChatJob({...job,epoch:'1',sequence:'600'},job),/Stale API request snapshot/);
  assert.throws(()=>validateChatJob({...job,state:'Completed'},job),/Invalid API request snapshot/);
});
