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
export function installHistory({older,newer,refresh,output,label,invoke,getEpoch}) {
  let group=null;
  function load(delta) {
    const epoch=getEpoch();if(epoch===null)return Promise.resolve();
    if(!group||group.epoch!==epoch)group={epoch,offset:0,desired:0,pending:null,promise:null};
    const current=group;
    current.desired=Math.max(0,Math.min(1024,current.desired+delta));
    current.pending=current.desired;
    if(current.promise)return current.promise;
    current.promise=(async()=>{
      while(current.pending!==null&&group===current&&getEpoch()===epoch){
        const requested=current.pending;current.pending=null;
        try {
          const snapshot=acceptHistory(await invoke('history_shell',{epoch,offset:requested}),epoch);
          if(group!==current||getEpoch()!==epoch)return;
          // A more recent user request supersedes this result; do not disturb
          // selected text with an intermediate page.
          if(current.pending!==null)continue;
          current.offset=current.desired=snapshot.offset;output.value=snapshot.text;output.setSelectionRange(0,0);
          const end=snapshot.total_rows-snapshot.offset,start=Math.max(1,end-127);
          label.textContent=`Snapshot ${snapshot.revision}: ${end>0?`archived rows ${start}–${end}`:'no archived rows'} of ${snapshot.total_rows}; ${snapshot.offset} newer rows skipped. Select text and copy normally. Refresh for new output.`;
        }catch(e){
          if(group===current&&getEpoch()===epoch){
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
