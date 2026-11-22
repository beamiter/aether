// Bounded immutable text snapshots. No ANSI parsing or clipboard privilege.
// UTF-16 length is a lower bound on UTF-8 bytes. Reject oversized strings
// before encoding so validation never allocates an unbounded second copy.
export function acceptHistory(value, epoch) {
  if (!value || value.epoch !== epoch || typeof value.revision !== 'string' || !/^\d{1,20}$/.test(value.revision) ||
      !Number.isInteger(value.offset) || value.offset < 0 || value.offset > 1024 ||
      !Number.isInteger(value.total_rows) || value.total_rows < 0 || value.total_rows > 1024 || value.offset > value.total_rows ||
      typeof value.text !== 'string' || value.text.length > 131200 || new TextEncoder().encode(value.text).length > 131200 || /[\x00-\x08\x0b-\x1f\x7f-\x9f]/.test(value.text)) throw new Error('invalid history snapshot');
  return Object.freeze({...value});
}
// Mirrors the fixed page size in LiveBridge::history (bridge/src/live.rs).
const HISTORY_PAGE_ROWS=128;
const oldestOffset=total=>Math.max(0,total-HISTORY_PAGE_ROWS);
export function installHistory({older,newer,refresh,output,label,invoke,getOwner}) {
  let group=null;
  function load(delta) {
    const owner=getOwner();if(owner===null)return Promise.resolve();
    if(group?.owner!==owner){
      const sameShell=group?.owner.session===owner.session&&group.owner.epoch===owner.epoch;
      // Recovery can rotate the attachment without replacing the PTY. Keep
      // only the page already shown, never its old pending navigation lease.
      const offset=sameShell?group.offset:0,total=sameShell?group.total:null;
      group={owner,offset,total,desired:offset,pending:null,promise:null};
    }
    const current=group;
    const isCurrent=()=>group===current&&getOwner()===owner;
    const maxOffset=current.total===null?1024:oldestOffset(current.total);
    const desired=Math.max(0,Math.min(maxOffset,current.desired+delta));
    // An already visible edge needs no IPC and must not disturb selection.
    // Refresh (delta zero) still explicitly requests current archive metadata.
    if(delta!==0&&desired===current.desired&&current.total!==null)return current.promise??Promise.resolve();
    current.desired=desired;
    current.pending={offset:desired,corrected:false};
    if(current.promise)return current.promise;
    current.promise=(async()=>{
      while(current.pending!==null&&isCurrent()){
        const {offset:requested,corrected}=current.pending;current.pending=null;
        try {
          const value=await invoke('history_shell',{owner,offset:requested});
          if(!isCurrent())return;
          // A newer request already owns the output. Do not validate or encode
          // the abandoned reply, and do not disturb selected text.
          if(current.pending!==null)continue;
          const snapshot=acceptHistory(value,owner.epoch);
          const max=oldestOffset(snapshot.total_rows);
          if(snapshot.offset>max){
            // The archive length was unknown or shrank after the last page.
            // Keep the visible page while obtaining one valid boundary snapshot.
            // Repeated shrink cannot spin; the next refresh is user-driven.
            if(corrected)throw new Error('Archive changed during navigation. Refresh to view archived rows.');
            current.desired=max;
            current.pending={offset:max,corrected:true};
            continue;
          }
          const unchanged=current.total===snapshot.total_rows&&current.offset===snapshot.offset&&output.value===snapshot.text;
          current.offset=current.desired=snapshot.offset;current.total=snapshot.total_rows;
          // Refresh can advance the revision without changing this page.
          // Assigning even identical text would collapse the browser selection.
          if(!unchanged){output.value=snapshot.text;output.setSelectionRange(0,0);}
          const end=snapshot.total_rows-snapshot.offset,start=Math.max(1,end-HISTORY_PAGE_ROWS+1);
          label.textContent=`Snapshot ${snapshot.revision}: ${end>0?`archived rows ${start}–${end}`:'no archived rows'} of ${snapshot.total_rows}; ${snapshot.offset} newer rows skipped. Select text and copy normally. Refresh for new output.`;
        }catch(e){
          if(isCurrent()){
            // A failed superseded request must not erase newer user intent.
            // Consume only that queued target, never retry this failure itself.
            if(current.pending!==null)continue;
            current.desired=current.offset;label.textContent=String(e?.message??e);
          }
          return;
        }
      }
    })().finally(()=>{current.promise=null;});
    return current.promise;
  }
  older.addEventListener('click',()=>load(20));newer.addEventListener('click',()=>load(-20));refresh.addEventListener('click',()=>load(0));
  return {reset(){group=null;output.value='';label.textContent='Snapshot cleared. Refresh to view current session.';}};
}
