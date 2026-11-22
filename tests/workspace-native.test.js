import test from 'node:test';
import assert from 'node:assert/strict';
import {validateNativeTask} from '../web/workspace-native.js';

const incarnation='11111111-1111-4111-8111-111111111111';
const approval={id:'22222222-2222-4222-8222-222222222222',kind:'Command',
  command:'printf fixture-only',cwd:'/tmp/managed',reason:null,file_changes:[]};
const task={
  identity:{task:'jterm:task:fixture',incarnation,generation:4},sequence:'6',
  title:'Native task',status:'Waiting for approval',worktree:'/tmp/managed',branch:'aether/fixture',
  detail:null,output:'Pending',truncated:false,controlled:true,can_stop:true,can_finish:false,
  can_inspect:false,draft_goal:null,approvals:[approval],
  verification:{recipe:null,shell:null,status:'Not run',output:'',truncated:false,can_start:false},
};

test('native approval snapshots are scoped to the current task, incarnation and generation',()=>{
  assert.equal(validateNativeTask(task),task);
  assert.throws(()=>validateNativeTask({...task,sequence:'5'},task),/Stale or foreign/);
  assert.throws(()=>validateNativeTask({...task,identity:{...task.identity,generation:3}},task),/Stale or foreign/);
  assert.throws(()=>validateNativeTask({...task,identity:{...task.identity,incarnation:approval.id}},task),/Stale or foreign/);
  assert.throws(()=>validateNativeTask({...task,approvals:[approval,approval]}),/Invalid native approval evidence/);
});

test('verification snapshots must include bounded output and owned recipe admission',()=>{
  const ready={recipe:'cargo test',shell:'/bin/sh',status:'Not run',output:'',truncated:false,can_start:true};
  assert.equal(validateNativeTask({...task,verification:ready}).verification,ready);
  assert.throws(()=>validateNativeTask({...task,verification:undefined}),/Invalid native task/);
  assert.throws(()=>validateNativeTask({...task,controlled:false,verification:ready}),/Invalid native task/);
  assert.throws(()=>validateNativeTask({...task,verification:{...ready,recipe:null}}),/Invalid native task/);
  assert.throws(()=>validateNativeTask({...task,verification:{...ready,shell:null}}),/Invalid native task/);
  assert.throws(()=>validateNativeTask({...task,verification:{...ready,output:'x'.repeat(65537)}}),/Invalid native task/);
});
