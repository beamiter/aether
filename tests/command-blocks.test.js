import test from 'node:test';
import assert from 'node:assert/strict';
import {validateCommandBlockSnapshot,validateFrozenReview} from '../web/command-blocks.js';

const owner={session:'a'.repeat(32),epoch:3,generation:2};
const block={
  sequence:'4',revision:'a'.repeat(64),execution_id:'execution-2',shell_session:null,shell_sequence:'2',
  command:'printf 文',cwd:'/tmp',exit:null,provenance:'shell_reported',
  output:{kind:'text',text:'文\n'},
};
const snapshot={owner,enabled:true,blocks:[block]};

test('command block records require exact attachment identity and bounded ordered observations',()=>{
  assert.doesNotThrow(()=>validateCommandBlockSnapshot(snapshot,owner));
  for(const change of [
    {owner:{...owner,generation:1}},
    {owner:{...owner,session:'b'.repeat(32)}},
    {owner:{...owner,epoch:4}},
    {enabled:false},
    {blocks:[block,block]},
    {blocks:[block,{...block,sequence:'6'}]},
    {blocks:[{...block,revision:'4'}]},
    {blocks:[{...block,provenance:'journal_recovered'}]},
    {blocks:[{...block,exit:2147483648}]},
    {blocks:[{...block,output:{kind:'text',text:'文'.repeat(5462)}}]},
  ])assert.throws(()=>validateCommandBlockSnapshot({...snapshot,...change},owner));
});

test('frozen output receipts bind to the exact local attachment',()=>{
  const receipt={owner,reviewId:'87dea754-7433-48a5-9c44-bcaf5137a207',
    complete:true,summary:'+1 added',comparison:'-old\n+new\n'};
  assert.doesNotThrow(()=>validateFrozenReview(receipt,owner));
  for(const changes of [{owner:{...owner,epoch:4}},{owner:{...owner,generation:3}},
    {reviewId:'local'},{comparison:'x'.repeat(512*1024+1)}]){
    assert.throws(()=>validateFrozenReview({...receipt,...changes},owner));
  }
});
