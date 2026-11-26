import test from 'node:test';
import fs from 'node:fs';
import {createHash} from 'node:crypto';
import {readLocalShellLayout as oldRead,saveLocalShellLayout as oldSave} from './fixtures/local-layout-v4.js';
import assert from 'node:assert/strict';
import {plainLocalTab,readLocalShellLayout,saveLocalShellLayout,MAX_LOCAL_SHELLS,localSplitPair} from '../web/local-layout.js';

test('the layout retains shell type, presentation, active position and split',()=>{
 const entries=new Map();
 const environment={localStorage:{setItem:(key,value)=>entries.set(key,value),
  getItem:key=>entries.get(key),removeItem:key=>entries.delete(key)}};
 const saved={tabs:[{shell:'jsh',presentation:'chat'},plainLocalTab],active:0,split:true};
 saveLocalShellLayout({...saved,tabs:[{...saved.tabs[0],session:'opaque lease'},plainLocalTab],
  command:'sensitive draft'},environment);
 assert.deepEqual([...entries.values()].map(JSON.parse),[{storageVersion:1,layout:{version:3,...saved},legacyToken:'absent'}]);
 assert.deepEqual(readLocalShellLayout(environment),saved);
 saveLocalShellLayout(null,environment);
 assert.equal(readLocalShellLayout(environment),null);
 assert.equal(entries.has('aether.local-shell-layout'),false);
 assert.equal(JSON.parse(entries.get('aether.local-shell-layout.v5')).layout,null);
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

test('saved panes keep their own identities and reject duplicate or plain-shell ownership',()=>{
 const paneId='6'.repeat(32);
 const tab={shell:'jsh',presentation:'chat',paneId};
 const layout={tabs:[tab,plainLocalTab],active:1,split:true};
 let raw='';
 const environment={localStorage:{setItem:(_,value)=>raw=value,getItem:()=>raw}};
 saveLocalShellLayout(layout,environment);
 assert.deepEqual(JSON.parse(raw).layout,{version:4,...layout});
 assert.deepEqual(readLocalShellLayout(environment),layout);
 for(const tabs of [[tab,tab],[{...plainLocalTab,paneId},tab],[{...tab,paneId:'../outside'}]]){
  raw=JSON.stringify({version:4,tabs,active:0,split:false});
  assert.equal(readLocalShellLayout(environment),null);
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


test('v5 restores three independent physical geometries and the saved visible pair only',()=>{
 let raw='';
 const environment={localStorage:{setItem:(_,value)=>raw=value,getItem:()=>raw}};
 const layout={tabs:[{...plainLocalTab,geometry:{rows:17,cols:71}},
  {...plainLocalTab,geometry:{rows:12,cols:83}},{shell:'jsh',presentation:'chat',paneId:'a'.repeat(32),geometry:{rows:19,cols:95}}],active:1,split:true,panes:[0,1]};
 saveLocalShellLayout({...layout,draft:'must not persist'},environment);
 assert.deepEqual(JSON.parse(raw).layout,{version:5,...layout});
 assert.deepEqual(readLocalShellLayout(environment),layout);
 assert(!raw.includes('must not persist'));
 for(const mutate of [
  v=>v.tabs[1].geometry.rows=0,v=>v.tabs[1].geometry.cols=257,v=>delete v.tabs[1].geometry,
  v=>v.panes=[0,0],v=>v.panes=[0,3],v=>v.active=2,v=>v.tabs[0].geometry.draft='secret',
  v=>v.tabs=Array(MAX_LOCAL_SHELLS+1).fill(layout.tabs[0]),
 ]){
  const value=structuredClone({version:5,...layout});mutate(value);
  assert.equal(readLocalShellLayout({localStorage:{getItem:()=>JSON.stringify(value)}}),null);
 }
});

test('a parked third tab replaces the focused split pane without changing the peer',()=>{
 assert.deepEqual(localSplitPair([10,20,30],30,[10,20],20),[10,30]);
 assert.deepEqual(localSplitPair([10,20,30],10,[10,30],30),[10,30]);
 assert.deepEqual(localSplitPair([10,20,30],20,[10,30],30),[10,20]);
 assert.deepEqual(localSplitPair([10],10,[10,20],20),[]);
 assert.deepEqual(localSplitPair([10,30],30,[20,30],20),[30,10]);
});

function keyedStorage(initial={}) {
 const entries=new Map(Object.entries(initial));
 return {entries,localStorage:{getItem:key=>entries.get(key)??null,
  setItem:(key,value)=>entries.set(key,value),removeItem:key=>entries.delete(key)}};
}
const legacyKey='aether.local-shell-layout',modernKey='aether.local-shell-layout.v5';
const modernThree=()=>({tabs:[{...plainLocalTab,geometry:{rows:17,cols:71}},
 {...plainLocalTab,geometry:{rows:12,cols:83}},{...plainLocalTab,geometry:{rows:19,cols:95}}],
 active:1,split:true,panes:[0,1]});
const legacyTwo=()=>({tabs:[plainLocalTab,{shell:'jsh',presentation:'chat'}],active:0,split:true});

test('the actual frozen pre-layout-fix reader retains its supported snapshot alongside modern tabs',()=>{
 const original=fs.readFileSync(new URL('./fixtures/local-layout-v4.js',import.meta.url));
 assert.equal(createHash('sha256').update(original).digest('hex'),'61624e55cb4a64f3165867f59b84c0cd86739c683583fb00f94e26f5c43b6a95');
 const env=keyedStorage();oldSave(legacyTwo(),env);
 const before=env.entries.get(legacyKey),old=oldRead(env);
 saveLocalShellLayout(modernThree(),env);
 assert.equal(env.entries.get(legacyKey),before,'new code must not overwrite the earlier release snapshot');
 assert.deepEqual(oldRead(env),old,'old code restores only its supported two tabs');
 assert.deepEqual(readLocalShellLayout(env),modernThree());
 assert.equal(oldRead(keyedStorage({[legacyKey]:JSON.stringify({version:5,...modernThree()})})),null,
  'this does not add >2-tab support to old code');
});

test('legacy fallbacks remain readable with absent, malformed and future modern records',()=>{
 const env=keyedStorage();oldSave(legacyTwo(),env);
 for(const raw of [null,'{bad',JSON.stringify({storageVersion:2,layout:{},legacyToken:'absent'}),'x'.repeat(4097)]){
  if(raw===null)env.entries.delete(modernKey);else env.entries.set(modernKey,raw);
  assert.deepEqual(readLocalShellLayout(env),legacyTwo());
 }
});

test('a genuine older writer change wins without mutating the preserved modern state',()=>{
 const env=keyedStorage();oldSave(legacyTwo(),env);saveLocalShellLayout(modernThree(),env);
 const intact=env.entries.get(modernKey),changed={...legacyTwo(),active:1};
 oldSave(changed,env);
 assert.deepEqual(readLocalShellLayout(env),changed);
 assert.equal(env.entries.get(modernKey),intact);
 // Once the user explicitly works in the newer app, its new checkpoint can
 // intentionally replace modern state while leaving that legacy snapshot intact.
 const legacy=env.entries.get(legacyKey);saveLocalShellLayout(changed,env);
 assert.deepEqual(readLocalShellLayout(env),changed);assert.equal(env.entries.get(legacyKey),legacy);
});

test('a semantic legacy version/key-order rewrite does not mask the modern layout',()=>{
 const env=keyedStorage({[legacyKey]:'{"version":1,"count":2,"active":0}'});
 saveLocalShellLayout(modernThree(),env);
 oldSave({tabs:[plainLocalTab,plainLocalTab],active:0,split:false},env);
 assert.deepEqual(readLocalShellLayout(env),modernThree());
});

test('old explicit clear is honored while retaining the modern backup',()=>{
 const env=keyedStorage();oldSave(legacyTwo(),env);saveLocalShellLayout(modernThree(),env);
 const intact=env.entries.get(modernKey);oldSave(null,env);
 assert.equal(readLocalShellLayout(env),null);assert.equal(env.entries.get(modernKey),intact);
});

test('new clear is a tombstone, removes legacy fallback, and admits a later legitimate old layout',()=>{
 const env=keyedStorage();oldSave(legacyTwo(),env);saveLocalShellLayout(modernThree(),env);
 saveLocalShellLayout(null,env);
 assert.equal(readLocalShellLayout(env),null);assert.equal(oldRead(env),null);
 assert.equal(JSON.parse(env.entries.get(modernKey)).layout,null);
 oldSave(legacyTwo(),env);
 assert.deepEqual(readLocalShellLayout(env),legacyTwo(),'a later deliberate older-app save is new intent');
});

test('failed legacy removal cannot resurrect its old tabs after modern clear',()=>{
 const env=keyedStorage();oldSave(legacyTwo(),env);saveLocalShellLayout(modernThree(),env);
 const legacy=env.entries.get(legacyKey);env.localStorage.removeItem=()=>{throw Error('blocked removal');};
 assert.doesNotThrow(()=>saveLocalShellLayout(null,env));
 assert.equal(env.entries.get(legacyKey),legacy);assert.equal(readLocalShellLayout(env),null);
});

test('unreadable legacy state is not treated as an intentional older-app clear',()=>{
 const env=keyedStorage();oldSave(legacyTwo(),env);saveLocalShellLayout(modernThree(),env);
 const get=env.localStorage.getItem;env.localStorage.getItem=key=>{if(key===legacyKey)throw Error('blocked read');return get(key);};
 assert.deepEqual(readLocalShellLayout(env),modernThree());
});

test('the already shipped single-slot v5 is a bounded migration fallback, not an old-reader claim',()=>{
 const env=keyedStorage({[legacyKey]:JSON.stringify({version:5,...modernThree()})});
 assert.deepEqual(readLocalShellLayout(env),modernThree());assert.equal(oldRead(env),null);
 saveLocalShellLayout(modernThree(),env);const intact=env.entries.get(modernKey);
 oldSave(legacyTwo(),env);
 assert.deepEqual(readLocalShellLayout(env),legacyTwo());assert.equal(env.entries.get(modernKey),intact);
});
