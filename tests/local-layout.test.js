import test from 'node:test';
import assert from 'node:assert/strict';
import {plainLocalTab,readLocalShellLayout,saveLocalShellLayout} from '../web/local-layout.js';

test('the layout retains shell type, presentation, active position and split',()=>{
 const entries=new Map();
 const environment={localStorage:{setItem:(key,value)=>entries.set(key,value),
  getItem:key=>entries.get(key),removeItem:key=>entries.delete(key)}};
 const saved={tabs:[{shell:'jsh',presentation:'chat'},plainLocalTab],active:0,split:true};
 saveLocalShellLayout({...saved,tabs:[{...saved.tabs[0],session:'opaque lease'},plainLocalTab],
  command:'sensitive draft'},environment);
 assert.deepEqual([...entries.values()].map(JSON.parse),[{version:3,...saved}]);
 assert.deepEqual(readLocalShellLayout(environment),saved);
 saveLocalShellLayout(null,environment);
 assert.equal(entries.size,0);
});

test('versions one and two restore plain tabs with their saved layout',()=>{
 const tabs=[plainLocalTab,plainLocalTab];
 for(const [raw,split] of [
  ['{"version":1,"count":2,"active":1}',false],
  ['{"version":2,"count":2,"active":1,"split":true}',true],
 ]){
  const environment={localStorage:{getItem:()=>raw}};
  assert.deepEqual(readLocalShellLayout(environment),{tabs,active:1,split});
 }
});

test('invalid, stale and unavailable preferences do not restore a shell layout',()=>{
 for(const raw of ['{bad', 'null', '{}', ' '.repeat(257),
   '{"version":0,"count":2,"active":1}', '{"version":1,"count":3,"active":1}',
   '{"version":1,"count":1,"active":1}', '{"version":1,"count":2,"active":0.5}',
   '{"version":1,"count":2,"active":0,"draft":"not allowed"}',
   '{"version":2,"count":1,"active":0,"split":true}',
   '{"version":2,"count":2,"active":0,"split":1}',
   '{"version":3,"tabs":[{"shell":"/tmp/script","presentation":"chat"}],"active":0,"split":false}',
   '{"version":3,"tabs":[{"shell":"jsh","presentation":"vte"}],"active":0,"split":false}',
   '{"version":3,"tabs":[{"shell":"jsh","presentation":"chat","draft":"text"}],"active":0,"split":false}']){
  assert.equal(readLocalShellLayout({localStorage:{getItem:()=>raw}}),null);
 }
 const unavailable={get localStorage(){throw Error('storage unavailable');}};
 assert.equal(readLocalShellLayout(unavailable),null);
 assert.doesNotThrow(()=>saveLocalShellLayout({tabs:[plainLocalTab],active:0,split:false},unavailable));
});
