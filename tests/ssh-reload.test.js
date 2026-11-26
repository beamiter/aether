import test from 'node:test';import assert from 'node:assert/strict';
import {validateReloadView,saveReloadTicket,takeReloadTicket} from '../web/ssh-reload.js';
const attachment=n=>({session:n.toString(16).padStart(32,'0'),epoch:n,generation:n+1});
const view=()=>({version:1,tabs:[1,2,3].map(n=>({owner:attachment(n),shell:n===2?'jsh':'plain',paneId:n===2?'a'.repeat(32):null,geometry:{rows:20+n,cols:80+n},draft:'retained '+n})),splitEnabled:true,splitEpochs:[3,1],detachedLocal:attachment(3),remoteDraft:'do not send'});
test('retained split rehydration preserves three exact identities, geometry, drafts and ordering',()=>{
 const original=view(),copy=validateReloadView(original);assert.deepEqual(copy,original);copy.tabs[0].draft='new';assert.equal(original.tabs[0].draft,'retained 1');
});
test('reload metadata rejects duplicate, foreign, unbounded and malformed state',()=>{
 for(const change of [v=>v.tabs.push(v.tabs[0]),v=>v.splitEpochs=[1,1],v=>v.splitEpochs=[1,99],v=>v.tabs[0].geometry.rows=129,v=>v.remoteDraft='x'.repeat(16385),v=>v.detachedLocal=attachment(7),v=>v.tabs[0].extra=true,v=>v.tabs[0].draft='\0']){
  const v=view();change(v);assert.throws(()=>validateReloadView(v));
 }
});
test('session storage contains only one opaque token and consumes it before a claim',()=>{
 const items=new Map(),storage={getItem:k=>items.get(k),setItem:(k,v)=>items.set(k,v),removeItem:k=>items.delete(k)};
 const ticket='f'.repeat(32);saveReloadTicket(ticket,storage);assert.deepEqual([...items.values()],[ticket]);assert.equal(takeReloadTicket(storage),ticket);assert.equal(takeReloadTicket(storage),null);assert.throws(()=>saveReloadTicket('old-owner',storage));
});

test('reader fallback filters dead panes without inventing owners or restoring remote input',async()=>{
 const {retainedReloadView}=await import('../web/ssh-reload.js');
 const kept=retainedReloadView(view(),[1,2]);assert.deepEqual(kept.tabs.map(tab=>tab.owner),[attachment(1),attachment(2)]);
 assert.equal(kept.tabs[0].draft,'retained 1');assert.equal(kept.remoteDraft,'');assert.equal(kept.splitEnabled,false);assert.deepEqual(kept.splitEpochs,[]);assert.equal(kept.detachedLocal,null);
 const split=retainedReloadView(view(),[1,2,3]);assert.deepEqual(split.splitEpochs,[3,1]);
 assert.throws(()=>retainedReloadView(view(),[1,1]));
});
test('a malformed one-use ticket is consumed and reported rather than mistaken for no handoff',()=>{
 const items=new Map([['aether.ssh-writer-reload','invalid']]),storage={getItem:k=>items.get(k),removeItem:k=>items.delete(k)};
 assert.throws(()=>takeReloadTicket(storage));assert.equal(items.size,0);
});
