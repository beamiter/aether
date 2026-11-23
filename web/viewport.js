import {CELL_WIDTH,CELL_HEIGHT} from './frames.js';

export function dimensions(rows,cols) {
  if(!Number.isInteger(rows)||!Number.isInteger(cols)||rows<1||rows>128||cols<1||cols>256)throw new Error('Use integer rows 1–128 and columns 1–256.');
  return {rows,cols};
}
// At most one IPC plus one latest pending resize per epoch. A stuck old epoch
// never blocks a new session; accepted writes are independent of resizing.
export function resizeQueue({getEpoch,invoke,onDone,onError}) {
  let group=null;
  return {request(rows,cols){
    const size=dimensions(rows,cols),epoch=getEpoch();if(epoch===null)return Promise.resolve();
    if(!group||group.epoch!==epoch)group={epoch,pending:null,promise:null};
    const current=group;current.pending=size;
    if(current.promise)return current.promise;
    current.promise=(async()=>{
      while(current.pending && group===current && getEpoch()===epoch){
        const next=current.pending;current.pending=null;
        try {await invoke('resize_shell',{epoch,...next});if(group===current&&getEpoch()===epoch&&!current.pending)await onDone(next);}
        catch(e){
          if(group!==current||getEpoch()!==epoch)break;
          onError(e);
          // Do not retry an uncertain resize or its failed completion callback.
          // A newer explicit absolute size is independent intent, not a retry.
          if(!current.pending)break;
        }
      }
    })().finally(()=>{current.promise=null;});
    return current.promise;
  }};
}
export function cursorTarget(frame,canvas,viewport) {
  const left=(canvas.offsetLeft||0)+frame.cursor_col*CELL_WIDTH;
  const top=(canvas.offsetTop||0)+frame.cursor_row*CELL_HEIGHT;
  return {left:Math.max(0,Math.min(left-viewport.clientWidth/2,viewport.scrollWidth-viewport.clientWidth)),
    top:Math.max(0,Math.min(top-viewport.clientHeight/2,viewport.scrollHeight-viewport.clientHeight))};
}

export function terminalCellAt(frame,canvas,x,y) {
  const rect=canvas.getBoundingClientRect();
  if(rect.width<=0||rect.height<=0||x<rect.left||y<rect.top||x>=rect.left+rect.width||y>=rect.top+rect.height)return null;
  return {col:Math.floor((x-rect.left)/rect.width*frame.cols),row:Math.floor((y-rect.top)/rect.height*frame.rows.length)};
}
