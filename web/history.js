import {acceptHistoryProjection,CHANNEL_IMAGE_VERSION,MAX_PROJECTED_HISTORY_ROWS,channelCounter} from './frames-v4.js';
import {CELL_WIDTH,CELL_HEIGHT,clearPaint,paint} from './frames.js';
import {terminalCellAt} from './viewport.js';

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

// A projected row starts at a raw history cell. A shared source plus exact
// intervening text identifies the same selection even when the page or the
// soft-wrap boundaries move. A matching string by itself could be another row.
function remapSelection(previous,next,start,end){
  if(!previous||!next||!(end>start))return null;
  const key=({row,column})=>`${row}:${column}`;
  const nextOffsets=new Map(next.sources.map((source,index)=>[key(source),next.offsets[index][0]]));
  let before,after;
  for(let index=0;index<previous.sources.length;index++){
    const oldOffset=previous.offsets[index][0];
    const newOffset=nextOffsets.get(key(previous.sources[index]));
    if(newOffset===undefined)continue;
    if(oldOffset<=start)before={oldOffset,newOffset};
    else{after={oldOffset,newOffset};break;}
  }
  for(const point of [before,after]){
    if(!point)continue;
    const {oldOffset,newOffset}=point,shift=newOffset-oldOffset;
    const first=Math.min(oldOffset,start),last=Math.max(oldOffset,end);
    if(first+shift<0||last+shift>next.text.length)continue;
    if(previous.text.slice(first,last)===next.text.slice(first+shift,last+shift))return [start+shift,end+shift];
  }
  return null;
}

function historyGrid({viewport,spacer,canvas},output) {
  const overscan = 3;
  let page = null, tile = null, pointer = null;

  function render() {
    if (!page?.rows.length) return;
    const top = Math.floor(viewport.scrollTop/CELL_HEIGHT);
    const visible = Math.ceil(viewport.clientHeight/CELL_HEIGHT)+1;
    const first = Math.max(0,top-overscan), end = Math.min(page.rows.length,top+visible+overscan);
    const start = output.selectionStart, stop = output.selectionEnd;
    const scale = globalThis.devicePixelRatio || 1;
    if (tile?.page === page && tile.first === first && tile.end === end && tile.start === start &&
        tile.stop === stop && tile.scale === scale) return;
    const rows = page.rows.slice(first,end);
    const selection = [];
    if (stop > start) for (let index = first; index < end; index++) {
      const bounds = page.offsets[index];
      let selected = -1, last = 0;
      for(let col=0;col+1<bounds.length;col++){
        if(bounds[col+1]<=bounds[col]||bounds[col+1]<=start||bounds[col]>=stop)continue;
        if(selected===-1)selected=col;
        last=col+page.rows[index][col].width;
      }
      if(selected!==-1)selection.push({row:index-first,first:selected,last});
    }
    canvas.style.top = `${first*CELL_HEIGHT}px`;
    const frame = {...page, rows, layout:page.revision, selection};
    paint(canvas,frame,scale);
    canvas.dataset.firstRow = String(first);
    tile = {page,frame,first,end,start,stop,scale};
  }

  function hit(event) {
    if (!tile) return null;
    const point = terminalCellAt(tile.frame,canvas,event.clientX,event.clientY);
    if (!point) return null;
    const row = point.row + tile.first;
    const cells = page.rows[row], offsets = page.offsets[row];
    if (point.col >= offsets.length-1) return null;
    const column = point.col - Number(cells[point.col].width===0);
    return {start:offsets[column],end:offsets[column+cells[column].width]};
  }

  function select(a,b) {
    output.setSelectionRange(Math.min(a.start,b.start),Math.max(a.end,b.end));
    render();
  }

  viewport.addEventListener('scroll',render);
  output.addEventListener('select',render);
  globalThis.addEventListener?.('resize',render);
  canvas.addEventListener('pointerdown',event=>{
    if (event.button!==0 || event.ctrlKey || event.metaKey) return;
    const point=hit(event);if(!point)return;
    event.preventDefault();
    pointer={id:event.pointerId,point};
    canvas.setPointerCapture?.(event.pointerId);
    output.focus({preventScroll:true});
    select(point,point);
  });
  canvas.addEventListener('pointermove',event=>{
    if(pointer?.id!==event.pointerId)return;
    const point=hit(event);if(point)select(pointer.point,point);
  });
  canvas.addEventListener('lostpointercapture',()=>{pointer=null;});
  return {
    show(next, match=null){
      page=next;tile=null;pointer=null;
      viewport.hidden=!page?.rows.length;
      if(viewport.hidden){clearPaint(canvas);return;}
      spacer.style.width=`${page.cols*CELL_WIDTH}px`;
      spacer.style.height=`${page.rows.length*CELL_HEIGHT}px`;
      if (match!==null) {
        const row=page.offsets.findIndex(bounds=>bounds.at(-1)>match);
        viewport.scrollTop=Math.max(0,row*CELL_HEIGHT-viewport.clientHeight/2);
      } else viewport.scrollTop=0;
      render();
    },
  };
}

export function installHistory({older,newer,refresh,output,label,invoke,getOwner,searchControls,copyControls,visual,enableProjection}) {
  let group=null,activeSearch=null,searchId=0,retained=null;
  const grid=visual?historyGrid(visual,output):null;
  const format=owner=>grid&&enableProjection?.(owner)?{frameVersion:CHANNEL_IMAGE_VERSION}:{};
  const sameSource=(a,b)=>a?.row===b?.row&&a?.column===b?.column;
  const sameShell=(a,b)=>a?.session===b?.session&&a?.epoch===b?.epoch;
  const expired='History source expired. Refresh to view retained rows.';
  const isExpired=(page,owner)=>page?.sources.length&&retained?.owner===owner&&
    channelCounter(page.sources[0].row)<retained.first;

  function updateCopy(){
    if(copyControls)copyControls.button.disabled=!(getOwner()&&output.selectionEnd>output.selectionStart);
  }

  async function copySelection(){
    const owner=getOwner(),text=output.value.slice(output.selectionStart,output.selectionEnd);
    if(!owner||!text)return;
    try{
      await copyControls.write(text);
      if(getOwner()===owner)copyControls.status.textContent='Selection copied.';
    }catch(error){
      if(getOwner()===owner)copyControls.status.textContent=String(error?.message??error);
    }
  }

  function dropPage(message){
    group=null;
    stopSearch();
    output.value='';
    grid?.show(null);
    label.textContent=message;
    if(copyControls)copyControls.status.textContent='';
    updateCopy();
  }

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
    current.busy=true;
    if(group)group={...group,desired:group.offset,pending:null,promise:null};
    updateSearchControls();
    searchControls.status.textContent='Searching retained history…';
    try{
      // A search visits at most 1,024 rows. Byte-limited windows still
      // contain at least 16 rows until the oldest window. Allow one extra
      // request for the remainder of the currently visible window.
      for(let steps=0;steps<65;steps++){
        const options=format(owner);
        const result=await invoke('search_local_history',{owner,...options,search:{id:current.id,query:current.query,restart}});
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
        const snapshot=acceptHistory(result.snapshot,owner.epoch);
        const projection=options.frameVersion?acceptHistoryProjection(result.projection,owner,true):null;
        if(projection&&(projection.text!==snapshot.text||projection.revision!==snapshot.revision||
            projection.offset!==snapshot.offset||projection.total_rows!==snapshot.total_rows||projection.rows.length!==snapshot.row_count))
          throw new Error('history search projection changed its source');
        if(isExpired(projection,owner)){dropPage(expired);return;}
        const {start,end}=result, page=projection??snapshot;
        if(!Number.isInteger(start)||!Number.isInteger(end)||start<0||end>page.text.length||
            end<=start||page.text.slice(start,end)!==current.query)throw new Error('invalid history search range');
        if(output.value!==page.text)output.value=page.text;
        output.focus();output.setSelectionRange(start,end);
        updateCopy();
        const lineHeight=parseFloat(globalThis.getComputedStyle?.(output)?.lineHeight);
        if(Number.isFinite(lineHeight)&&output.clientHeight){
          const line=page.text.slice(0,start).split('\n').length-1;
          output.scrollTop=Math.max(0,line*lineHeight-output.clientHeight/2);
        }
        grid?.show(projection,start);
        label.textContent=`History snapshot ${page.revision}. Select or copy the matching text.`;
        searchControls.status.textContent='Match selected in retained history.';
        group={owner,offset:page.offset,total:page.total_rows,rows:snapshot.row_count??HISTORY_PAGE_ROWS,desired:page.offset,pending:null,promise:null,projection,search:true};
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
      const retainedShell=sameShell(group?.owner,owner);
      // Recovery can rotate the attachment without replacing the PTY. Keep
      // only the page already shown, never its old pending navigation lease.
      const offset=retainedShell?group.offset:0,total=retainedShell?group.total:null;
      const rows=retainedShell?group.rows:HISTORY_PAGE_ROWS;
      group={owner,offset,total,rows,desired:offset,pending:null,promise:null,projection:retainedShell?group.projection:null};
    }
    const current=group;
    const isCurrent=()=>group===current&&getOwner()===owner;
    const options=format(owner);
    const maxOffset=current.total===null?(options.frameVersion?MAX_PROJECTED_HISTORY_ROWS:1024):oldestOffset(current.total,current.rows);
    const step=Math.sign(delta)*Math.min(Math.abs(delta),Math.max(1,current.rows));
    const desired=Math.max(0,Math.min(maxOffset,current.desired+step));
    // An already visible edge needs no IPC and must not disturb selection.
    // Refresh (delta zero) still explicitly requests current archive metadata.
    if(delta!==0&&desired===current.desired&&current.total!==null)return current.promise??Promise.resolve();
    current.desired=desired;
    current.pending={offset:desired,corrected:false,centerSelection:delta===0};
    if(current.promise)return current.promise;
    current.promise=(async()=>{
      while(current.pending!==null&&isCurrent()){
        const {offset:requested,corrected,centerSelection}=current.pending;current.pending=null;
        try {
          const source=current.projection?.sources[0];
          const select=output.selectionEnd>output.selectionStart;
          const anchored=options.frameVersion&&source&&(requested>0||current.offset===0&&select);
          const position=anchored?{anchor:source,shift:current.offset-requested}:{};
          const value=await invoke('history_shell',{owner,offset:requested,...options,...position});
          if(!isCurrent())return;
          // A newer request already owns the output. Do not validate or encode
          // the abandoned reply, and do not disturb selected text.
          if(current.pending!==null)continue;
          const projection=options.frameVersion?acceptHistoryProjection(value,owner):null;
          if(isExpired(projection,owner)){dropPage(expired);return;}
          const snapshot=projection??acceptHistory(value,owner.epoch);
          const rows=projection?.rows.length??snapshot.row_count??HISTORY_PAGE_ROWS;
          const max=oldestOffset(snapshot.total_rows,rows);
          if(snapshot.offset>max){
            // The archive length was unknown or shrank after the last page.
            // Keep the visible page while obtaining one valid boundary snapshot.
            // Repeated shrink cannot spin; the next refresh is user-driven.
            if(corrected)throw new Error('Archive changed during navigation. Refresh to view archived rows.');
            current.desired=max;
            current.pending={offset:max,corrected:true,centerSelection};
            continue;
          }
          const samePage=projection?sameSource(current.projection?.sources[0],projection.sources[0]):
            current.total===snapshot.total_rows&&current.offset===snapshot.offset;
          const unchanged=samePage&&output.value===snapshot.text;
          // Read the live selection after the IPC. A drag made while the
          // snapshot was in flight should be the one adopted by the new page.
          const range=unchanged?null:remapSelection(current.projection,projection,output.selectionStart,output.selectionEnd);
          const direction=output.selectionDirection;
          current.offset=current.desired=snapshot.offset;current.total=snapshot.total_rows;current.rows=rows;
          current.projection=projection;current.search=false;
          // Refresh can advance the revision without changing this page.
          // Assigning even identical text would collapse the browser selection.
          if(!unchanged){
            output.value=snapshot.text;
            if(range)output.setSelectionRange(range[0],range[1],direction);
            else output.setSelectionRange(0,0);
          }
          const selected=output.selectionEnd>output.selectionStart;
          grid?.show(projection,centerSelection&&selected?output.selectionStart:null);
          updateCopy();
          const end=snapshot.total_rows-snapshot.offset,start=Math.max(1,end-rows+1);
          label.textContent=`Snapshot ${snapshot.revision}: ${end>0?`archived rows ${start}–${end}`:'no archived rows'} of ${snapshot.total_rows}; ${snapshot.offset} newer rows skipped. Select text and copy normally. Refresh for new output.`;
        }catch(e){
          if(isCurrent()){
            // A failed superseded request must not erase newer user intent.
            // Consume only that queued target, never retry this failure itself.
            if(current.pending!==null)continue;
            const message=String(e?.message??e);
            if(message===expired){dropPage(message);return;}
            current.desired=current.offset;label.textContent=message;
          }
          return;
        }
      }
    })().finally(()=>{current.promise=null;});
    return current.promise;
  }
  older.addEventListener('click',()=>load(20));newer.addEventListener('click',()=>load(-20));refresh.addEventListener('click',()=>load(0));
  if(copyControls){
    copyControls.button.addEventListener('click',copySelection);
    output.addEventListener('select',updateCopy);
    output.addEventListener('keydown',event=>{
      if((event.ctrlKey||event.metaKey)&&!event.altKey&&!event.shiftKey&&event.key.toLowerCase()==='c'&&
          output.selectionEnd>output.selectionStart){
        event.preventDefault();void copySelection();
      }
    });
    updateCopy();
  }
  if(searchControls){
    searchControls.form.addEventListener('submit',event=>{event.preventDefault();return find(true);});
    searchControls.older.addEventListener('click',()=>find(false));
    searchControls.cancel.addEventListener('click',()=>stopSearch('Search canceled.'));
    searchControls.query.addEventListener('input',()=>stopSearch());
    updateSearchControls();
  }
  return {
    updateSearchControls,
    dismiss(){
      stopSearch();
      // Preserve the visible page, but retire any outstanding page request.
      if(group)group={...group,desired:group.offset,pending:null,promise:null};
    },
    updateRetained(owner,first){
      retained={owner,first:channelCounter(first)};
      if(sameShell(group?.owner,owner)&&isExpired(group.projection,owner))dropPage(expired);
    },
    refreshProjection(){if(group?.projection&&!group.search&&!activeSearch?.busy)return load(0);},
    reset(){dropPage('Snapshot cleared. Refresh to view current session.');},
  };
}
