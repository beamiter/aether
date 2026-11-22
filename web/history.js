// Bounded immutable text snapshots. No ANSI parsing or clipboard privilege.
// UTF-16 length is a lower bound on UTF-8 bytes. Reject oversized strings
// before encoding so validation never allocates an unbounded second copy.
export function acceptHistory(value, epoch) {
  if (!value || value.epoch !== epoch || typeof value.revision !== 'string' || !/^\d{1,20}$/.test(value.revision) ||
      !Number.isInteger(value.offset) || value.offset < 0 || value.offset > 1024 ||
      !Number.isInteger(value.total_rows) || value.total_rows < 0 || value.total_rows > 1024 || value.offset > value.total_rows ||
      typeof value.text !== 'string' || value.text.length > 131200 || new TextEncoder().encode(value.text).length > 131200 || /[\x00-\x08\x0b-\x1f\x7f-\x9f]/.test(value.text)) throw new Error('invalid history snapshot');
  const count=value.row_count;
  if(count!==undefined&&(!Number.isInteger(count)||count<Number(value.total_rows>0)||
      count>Math.min(HISTORY_PAGE_ROWS,value.total_rows-value.offset)))throw new Error('invalid history snapshot row count');
  return Object.freeze({...value});
}
// Maximum ordinary page size in LiveBridge::history.
const HISTORY_PAGE_ROWS=128;
const oldestOffset=(total,rows=HISTORY_PAGE_ROWS)=>Math.max(0,total-rows);
export function installHistory({older,newer,refresh,output,label,invoke,getOwner,searchControls}) {
  let group=null,activeSearch=null,searchId=0;

  function updateSearchControls(){
    if(!searchControls)return;
    const owner=getOwner(),enabled=owner!==null&&searchControls.enabled(owner);
    if(activeSearch&&activeSearch.owner!==owner){
      activeSearch=null;
      searchControls.status.textContent='Search again in the current view.';
    }
    searchControls.form.hidden=!enabled;
    searchControls.find.disabled=!enabled||!!activeSearch?.busy;
    searchControls.older.disabled=!enabled||!activeSearch?.hasHit||activeSearch.busy;
    searchControls.cancel.disabled=!enabled||!activeSearch?.busy;
  }

  function stopSearch(message=''){
    activeSearch=null;
    if(searchControls)searchControls.status.textContent=message;
    updateSearchControls();
  }

  async function find(restart){
    const owner=getOwner();
    if(owner===null||!searchControls.enabled(owner)||activeSearch?.busy)return;
    if(restart){
      const query=searchControls.query.value;
      if(!query||query.length>128||new TextEncoder().encode(query).length>128||/[\x00-\x1f\x7f-\x9f]/.test(query)){
        stopSearch('Enter a literal history query of 1–128 UTF-8 bytes without control characters.');
        return;
      }
      activeSearch={id:++searchId,owner,query,busy:false,hasHit:false};
    }
    const current=activeSearch;
    if(!current||current.owner!==owner||(!restart&&!current.hasHit))return;
    const isCurrent=()=>activeSearch===current&&getOwner()===owner;
    current.busy=true;group=null;updateSearchControls();
    searchControls.status.textContent='Searching retained history…';
    try{
      // A search visits at most 1,024 rows. Byte-limited windows still
      // contain at least 16 rows until the oldest window. Allow one extra
      // request for the remainder of the currently visible window.
      for(let steps=0;steps<65;steps++){
        const result=await invoke('search_local_history',{owner,search:{id:current.id,query:current.query,restart}});
        if(!isCurrent())return;
        restart=false;
        if(!result||result.id!==current.id||result.epoch!==owner.epoch)throw new Error('invalid history search response');
        if(result.kind==='more')continue;
        if(result.kind==='end'){
          searchControls.status.textContent=current.hasHit?'No older match in retained history.':'No matches in retained history.';
          current.hasHit=false;
          return;
        }
        if(result.kind!=='match')throw new Error('invalid history search response');
        const page=acceptHistory(result.snapshot,owner.epoch);
        const {start,end}=result;
        if(!Number.isInteger(start)||!Number.isInteger(end)||start<0||end>page.text.length||
            end<=start||page.text.slice(start,end)!==current.query)throw new Error('invalid history search range');
        if(output.value!==page.text)output.value=page.text;
        output.focus();output.setSelectionRange(start,end);
        const lineHeight=parseFloat(globalThis.getComputedStyle?.(output)?.lineHeight);
        if(Number.isFinite(lineHeight)&&output.clientHeight){
          const line=page.text.slice(0,start).split('\n').length-1;
          output.scrollTop=Math.max(0,line*lineHeight-output.clientHeight/2);
        }
        label.textContent=`History snapshot ${page.revision}. Select or copy the matching text.`;
        searchControls.status.textContent='Match selected in retained history.';
        group={owner,offset:page.offset,total:page.total_rows,rows:page.row_count??HISTORY_PAGE_ROWS,desired:page.offset,pending:null,promise:null};
        current.hasHit=true;
        return;
      }
      throw new Error('History search exceeded its page limit.');
    }catch(error){
      if(isCurrent())stopSearch(String(error?.message??error));
    }finally{
      if(isCurrent()){current.busy=false;updateSearchControls();}
    }
  }

  function load(delta) {
    const owner=getOwner();if(owner===null)return Promise.resolve();
    stopSearch();
    if(group?.owner!==owner){
      const sameShell=group?.owner.session===owner.session&&group.owner.epoch===owner.epoch;
      // Recovery can rotate the attachment without replacing the PTY. Keep
      // only the page already shown, never its old pending navigation lease.
      const offset=sameShell?group.offset:0,total=sameShell?group.total:null;
      const rows=sameShell?group.rows:HISTORY_PAGE_ROWS;
      group={owner,offset,total,rows,desired:offset,pending:null,promise:null};
    }
    const current=group;
    const isCurrent=()=>group===current&&getOwner()===owner;
    const maxOffset=current.total===null?1024:oldestOffset(current.total,current.rows);
    const step=Math.sign(delta)*Math.min(Math.abs(delta),Math.max(1,current.rows));
    const desired=Math.max(0,Math.min(maxOffset,current.desired+step));
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
          const rows=snapshot.row_count??HISTORY_PAGE_ROWS;
          const max=oldestOffset(snapshot.total_rows,rows);
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
          current.offset=current.desired=snapshot.offset;current.total=snapshot.total_rows;current.rows=rows;
          // Refresh can advance the revision without changing this page.
          // Assigning even identical text would collapse the browser selection.
          if(!unchanged){output.value=snapshot.text;output.setSelectionRange(0,0);}
          const end=snapshot.total_rows-snapshot.offset,start=Math.max(1,end-rows+1);
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
  if(searchControls){
    searchControls.form.addEventListener('submit',event=>{event.preventDefault();return find(true);});
    searchControls.older.addEventListener('click',()=>find(false));
    searchControls.cancel.addEventListener('click',()=>stopSearch('Search canceled.'));
    searchControls.query.addEventListener('input',()=>stopSearch());
    updateSearchControls();
  }
  return {
    updateSearchControls,
    reset(){group=null;stopSearch();output.value='';label.textContent='Snapshot cleared. Refresh to view current session.';},
  };
}
