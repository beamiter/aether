// A reload carries only a one-use native ticket in sessionStorage. Drafts and
// retained pane metadata stay in native memory, never in the saved SSH catalog.
const key='aether.ssh-writer-reload';
const hex=value=>typeof value==='string'&&/^[a-f0-9]{32}$/.test(value);
const uint=value=>Number.isInteger(value)&&value>0&&value<=0xffffffff;
const keys=(value,names)=>value&&typeof value==='object'&&!Array.isArray(value)&&Object.keys(value).sort().join(',')===names.slice().sort().join(',');
const owner=value=>keys(value,['session','epoch','generation'])&&hex(value.session)&&uint(value.epoch)&&uint(value.generation);
const draft=value=>typeof value==='string'&&!value.includes('\0')&&new TextEncoder().encode(value).length<=16384;
export function validateReloadView(value){
 const fail=()=>{throw new Error('Invalid retained terminal layout.');};
 if(!keys(value,['version','tabs','splitEnabled','splitEpochs','detachedLocal','remoteDraft'])||value.version!==1||
    !Array.isArray(value.tabs)||value.tabs.length>8||typeof value.splitEnabled!=='boolean'||
    !Array.isArray(value.splitEpochs)||!draft(value.remoteDraft))fail();
 const epochs=new Set(),sessions=new Set(),panes=new Set();
 for(const tab of value.tabs){
  if(!keys(tab,['owner','shell','paneId','geometry','draft'])||!owner(tab.owner)||
     epochs.has(tab.owner.epoch)||sessions.has(tab.owner.session)||!['plain','jsh'].includes(tab.shell)||!draft(tab.draft)||
     tab.paneId!==null&&(!hex(tab.paneId)||tab.shell!=='jsh'||panes.has(tab.paneId))||
     tab.geometry!==null&&(!keys(tab.geometry,['rows','cols'])||!Number.isInteger(tab.geometry.rows)||tab.geometry.rows<1||tab.geometry.rows>128||
       !Number.isInteger(tab.geometry.cols)||tab.geometry.cols<1||tab.geometry.cols>256))fail();
  epochs.add(tab.owner.epoch);sessions.add(tab.owner.session);if(tab.paneId!==null)panes.add(tab.paneId);
 }
 if(value.splitEpochs.length!==(value.splitEnabled?2:0)||new Set(value.splitEpochs).size!==value.splitEpochs.length||
    value.splitEpochs.some(epoch=>!epochs.has(epoch))||value.detachedLocal!==null&&(!owner(value.detachedLocal)||
      !value.tabs.some(tab=>tab.owner.session===value.detachedLocal.session&&tab.owner.epoch===value.detachedLocal.epoch&&tab.owner.generation===value.detachedLocal.generation)))fail();
 return JSON.parse(JSON.stringify(value));
}
export function saveReloadTicket(ticket,storage=globalThis.sessionStorage){
 if(!hex(ticket)||!storage)throw new Error('One-use SSH recovery storage is unavailable.');
 storage.setItem(key,ticket);
 if(storage.getItem(key)!==ticket)throw new Error('One-use SSH recovery storage is unavailable.');
}
export function takeReloadTicket(storage=globalThis.sessionStorage){
 if(!storage)return null;
 const ticket=storage.getItem(key);storage.removeItem(key);
 if(ticket!==null&&ticket!==undefined&&!hex(ticket))throw new Error('The one-use SSH recovery ticket is invalid.');
 return ticket??null;
}

// Retained epochs come from the native manager, never from the saved launch
// layout. Dead panes are removed rather than reopened or granted a new owner.
export function retainedReloadView(value,epochs){
 const view=validateReloadView(value);
 if(!Array.isArray(epochs)||epochs.length>8||epochs.some(epoch=>!uint(epoch))||new Set(epochs).size!==epochs.length)throw new Error('Invalid retained local shell list.');
 view.tabs=view.tabs.filter(tab=>epochs.includes(tab.owner.epoch));
 const kept=new Set(view.tabs.map(tab=>tab.owner.epoch));
 if(view.splitEpochs.some(epoch=>!kept.has(epoch))){view.splitEnabled=false;view.splitEpochs=[];}
 if(view.detachedLocal&&!kept.has(view.detachedLocal.epoch))view.detachedLocal=null;
 view.remoteDraft='';
 return validateReloadView(view);
}
