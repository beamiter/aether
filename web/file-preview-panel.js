import {createFilePreview,selectedPathAtCell} from './file-preview.js';
// This panel is explicitly local and independent of terminal/Codex output.
// Directory selection grants a snapshot, not arbitrary path or remote access.
export function installFilePreviewPanel(document,{getFrame=()=>null,getEpoch=()=>null}={}){
 let paths=[],origin=null,selectionError=false;
 const picker=document.querySelector('#preview-folder'),choices=document.querySelector('#preview-files'),panel=document.querySelector('#file-preview'),title=document.querySelector('#preview-title'),body=document.querySelector('#preview-source'),notice=document.querySelector('#preview-notice');
 const preview=createFilePreview({render(item){
  panel.hidden=!item;title.textContent=item?item.path:'';body.textContent=item?item.text:'';
 },onError(message){selectionError=true;notice.textContent=message;}});
 const single=document.querySelector('#preview-single');
 function populate(selected){
  origin=null;choices.replaceChildren();
  for(const path of selected){const option=document.createElement('option');option.value=path;option.textContent=path;choices.append(option);}
  choices.disabled=!selected.length;document.querySelector('#preview-show').disabled=!selected.length;
 }
 picker.addEventListener('change',()=>{
  selectionError=false;single.value='';paths=preview.select(picker.files);populate(paths);
  if(!selectionError)notice.textContent=paths.length?`${paths.length} local text files available. Contents are a selected snapshot, not live disk or remote session data.`:'No supported local text files selected. Raster images are not enabled in this slice.';
 });
 single.addEventListener('change',()=>{
  selectionError=false;picker.value='';paths=[];
  // Do not guess a project-relative path from a basename. Single-file mode
  // is manual-only and cannot be selected by untrusted terminal references.
  const selected=single.files?.length===1?preview.selectFile(single.files[0]):(preview.clear(),[]);
  populate(selected);
  if(!selectionError)notice.textContent=selected.length?'One explicit local file selected. Use Preview selected source. Ctrl-click matching requires a bounded folder snapshot.':'No single file selected.';
 });
 choices.addEventListener('change',()=>{origin=null;preview.dismiss();});
 async function show(){if(origin!==null&&origin!==getEpoch()){preview.dismiss();notice.textContent='Terminal session changed. Select a local file explicitly again.';origin=null;return;}notice.textContent='';await preview.show(choices.value);}
 document.querySelector('#preview-show').addEventListener('click',show);
 document.querySelector('#preview-confirm-local').addEventListener('click',show);
 document.querySelector('#preview-close').addEventListener('click',()=>preview.dismiss());
 document.querySelector('#preview-clear').addEventListener('click',()=>{
  preview.clear();paths=[];origin=null;picker.value='';single.value='';choices.replaceChildren();choices.disabled=true;document.querySelector('#preview-show').disabled=true;notice.textContent='Local file access cleared.';
 });
 document.querySelector('canvas').addEventListener('click',event=>{
  if(event.button!==0||!event.ctrlKey||event.altKey||event.shiftKey||event.metaKey||event.defaultPrevented)return;
  const frame=getFrame(),epoch=getEpoch();if(epoch===null||!frame||frame.epoch!==epoch||frame.session!=='pty-demo')return;
  const rect=event.currentTarget.getBoundingClientRect();
  if(!rect.width||!rect.height)return;
  const column=Math.floor((event.clientX-rect.left)/rect.width*frame.cols),row=Math.floor((event.clientY-rect.top)/rect.height*frame.rows.length);
  const path=selectedPathAtCell(frame,row,column,paths);if(!path)return;
  event.preventDefault();preview.dismiss();origin=epoch;choices.value=path;
  // Output cannot assert local/remote identity. Require an explicit local
  // confirmation before reading a same-named selected file, even after Ctrl-click.
  panel.hidden=false;title.textContent=path;body.textContent='';
  notice.textContent='Terminal reference selected. Verify it refers to this local folder, not a remote host. Use Preview selected source to confirm the local file; no bytes have been read.';
  document.querySelector('#preview-confirm-local').focus();
 });
 return preview;
}

