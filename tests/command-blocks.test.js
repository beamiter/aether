import test from 'node:test';
import assert from 'node:assert/strict';
import {validateCommandBlockSnapshot,validateFrozenReview} from '../web/command-blocks.js';

const owner={session:'a'.repeat(32),epoch:3,generation:2};
const block={
  sequence:'4',revision:'a'.repeat(64),execution_id:'execution-2',shell_session:null,shell_sequence:'2',
  command:'printf 文',cwd:'/tmp',exit:null,provenance:'shell_reported',
  output:{kind:'text',text:'文\n'},
};
const snapshot={owner,enabled:true,blocks:[block],bookmarked:[]};

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

test('command sources preserve raw 64-bit offsets and ordered marker events',()=>{
  const source={
    byte_start:'18446744073709551604',byte_end:'18446744073709551615',
    start_event:'3',end_event:'4',start_size:[80,24],end_size:[80,24],resized:false,
  };
  const read=output_source=>validateCommandBlockSnapshot({
    ...snapshot,blocks:[{...block,output_source}],
  },owner);
  assert.doesNotThrow(()=>read(source));
  const identity={stream_id:'b'.repeat(32),execution_id:'c'.repeat(64)};
  assert.doesNotThrow(()=>read({...source,...identity}));
  for(const override of [
    {byte_start:Number(source.byte_start)},{byte_end:'18446744073709551616'},
    {byte_end:'18446744073709551603'},{end_event:'3'},{start_event:'0'},
    {end_size:[81,24]},{start_size:[80,0]},
    {stream_id:identity.stream_id},{execution_id:identity.execution_id},
    {...identity,stream_id:'shell-reported-id'},{...identity,execution_id:'7'},
  ])assert.throws(()=>read({...source,...override}));
  assert.doesNotThrow(()=>read({...source,end_size:[81,24],resized:true}));
});

test('command bookmarks reference only retained records and contain no duplicates',()=>{
  const blocks=[{...block,sequence:'5'},block];
  assert.doesNotThrow(()=>validateCommandBlockSnapshot({...snapshot,blocks,bookmarked:['4']},owner));
  for(const bookmarked of [['3'],[4],['4','4'],['4','5','3']]){
    assert.throws(()=>validateCommandBlockSnapshot({...snapshot,blocks,bookmarked},owner));
  }
});

test('styled output uses UTF-8 byte ranges independent of the displayed characters',()=>{
  const output_annotations={spans:[{bytes:{start:0,end:3},colors:[0x021fabc1,0,0],flags:65}],links:[]};
  const check=overrides=>validateCommandBlockSnapshot({
    ...snapshot,blocks:[{...block,output_annotations,...overrides}],
  },owner);
  assert.doesNotThrow(()=>check({}));
  assert.throws(()=>check({output:{kind:'screen_operation'}}));
  for(const span of [
    {bytes:{start:1,end:3},colors:[0,0,0],flags:0},
    {bytes:{start:0,end:3},colors:[0x01000107,0,0],flags:0},
    {bytes:{start:0,end:3},colors:[0,0,0],flags:384},
  ])assert.throws(()=>check({output_annotations:{spans:[span],links:[]}}));
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
