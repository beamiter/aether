import test from 'node:test';
import assert from 'node:assert/strict';
import {readLocalShellLayout,saveLocalShellLayout} from '../web/local-layout.js';

test('the versioned layout saves tab count, active position and split mode',()=>{
 const entries=new Map();
 const environment={localStorage:{setItem:(key,value)=>entries.set(key,value),
  getItem:key=>entries.get(key),removeItem:key=>entries.delete(key)}};
 saveLocalShellLayout({count:2,active:0,split:true,command:'sensitive draft',session:'opaque lease'},environment);
 assert.deepEqual([...entries.values()].map(JSON.parse),[{version:2,count:2,active:0,split:true}]);
 assert.deepEqual(readLocalShellLayout(environment),{count:2,active:0,split:true});
 saveLocalShellLayout(null,environment);
 assert.equal(entries.size,0);
});

test('version one remains readable as a single-pane tabbed layout',()=>{
 const environment={localStorage:{getItem:()=>'{"version":1,"count":2,"active":1}'}};
 assert.deepEqual(readLocalShellLayout(environment),{count:2,active:1,split:false});
});

test('invalid, stale and unavailable preferences do not restore a shell layout',()=>{
 for(const raw of ['{bad', 'null', '{}', ' '.repeat(97),
   '{"version":0,"count":2,"active":1}', '{"version":1,"count":3,"active":1}',
   '{"version":1,"count":1,"active":1}', '{"version":1,"count":2,"active":0.5}',
   '{"version":1,"count":2,"active":0,"draft":"not allowed"}',
   '{"version":2,"count":1,"active":0,"split":true}',
   '{"version":2,"count":2,"active":0,"split":1}']){
  assert.equal(readLocalShellLayout({localStorage:{getItem:()=>raw}}),null);
 }
 const unavailable={get localStorage(){throw Error('storage unavailable');}};
 assert.equal(readLocalShellLayout(unavailable),null);
 assert.doesNotThrow(()=>saveLocalShellLayout({count:1,active:0},unavailable));
});
