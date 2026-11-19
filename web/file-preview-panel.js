import {createFilePreview,selectedPathAtCell} from './file-preview.js';
// This panel is explicitly local and independent of terminal/Codex output.
// Directory selection grants a snapshot, not arbitrary path or remote access.
export function installFilePreviewPanel(document,{getFrame=()=>null,getEpoch=()=>null}={}){
 let paths=[],origin=null,selectionError=false;
 const picker=document.querySelector('#preview-folder'),choices=document.querySelector('#preview-files'),panel=document.querySelector('#file-preview'),title=document.querySelector('#preview-title'),body=document.querySelector('#preview-source'),notice=document.querySelector('#preview-notice');
 const raster=document.querySelector('#preview-raster');
 const preview=createFilePreview({render(item){
  raster.width=1;raster.height=1;raster.hidden=true;
  if(item?.bitmap){raster.width=item.width;raster.height=item.height;const context=raster.getContext('2d');if(!context)throw Error('Image canvas unavailable.');context.drawImage(item.bitmap,0,0);raster.hidden=false;}
  panel.hidden=!item;title.textContent=item?item.path:'';body.textContent=item?.bitmap?'Local image preview':item?item.text:'';
 },onError(message){selectionError=true;notice.textContent=message;}});
 const single=document.querySelector('#preview-single');
 function populate(selected){
  origin=null;choices.replaceChildren();
  for(const path of selected){const option=document.createElement('option');option.value=path;option.textContent=path;choices.append(option);}
  choices.disabled=!selected.length;document.querySelector('#preview-show').disabled=!selected.length;
 }
 picker.addEventListener('change',()=>{
  selectionError=false;single.value='';paths=preview.select(picker.files);populate(paths);
  if(!selectionError)notice.textContent=paths.length?`${paths.length} local preview files available. Contents are a selected snapshot, not live disk or remote session data.`:'No supported local source or PNG/JPEG files selected.';
 });
 single.addEventListener('change',()=>{
  selectionError=false;picker.value='';paths=[];
  // Do not guess a project-relative path from a basename. Single-file mode
  // is manual-only and cannot be selected by untrusted terminal references.
  const selected=single.files?.length===1?preview.selectFile(single.files[0]):(preview.clear(),[]);
  populate(selected);
  if(!selectionError)notice.textContent=selected.length?'One explicit local file selected. Use Preview selected file. Ctrl-click matching requires a bounded folder snapshot.':'No single file selected.';
 });
 choices.addEventListener('change',()=>{origin=null;preview.dismiss();});
 async function show(){if(origin!==null&&origin!==getEpoch()){preview.dismiss();notice.textContent='Terminal session changed. Select a local file explicitly again.';return;}notice.textContent='';await preview.show(choices.value);}
 document.querySelector('#preview-show').addEventListener('click',show);
 document.querySelector('#preview-confirm-local').addEventListener('click',show);
 function closeFromPanel(fromEscape=false){
  if(panel.hidden)return;
  preview.dismiss();notice.textContent='Preview closed. No terminal input sent.';
  // Escape stays away from the terminal so held-key repeat cannot send PTY
  // input. Explicit Close may restore only the same terminal session.
  const target=!fromEscape&&origin!==null&&origin===getEpoch()?document.querySelector('canvas'):document.querySelector('#preview-show');
  target.focus();
 }
 document.querySelector('#preview-close').addEventListener('click',()=>closeFromPanel());
 panel.addEventListener('keydown',event=>{
  if(panel.hidden||!panel.contains(document.activeElement)||event.key!=='Escape'||event.defaultPrevented||event.isComposing||event.keyCode===229||event.ctrlKey||event.altKey||event.shiftKey||event.metaKey||event.getModifierState?.('AltGraph'))return;
  event.preventDefault();event.stopPropagation();closeFromPanel(true);
 });
 document.querySelector('#preview-clear').addEventListener('click',()=>{
  preview.clear();paths=[];origin=null;picker.value='';single.value='';choices.replaceChildren();choices.disabled=true;document.querySelector('#preview-show').disabled=true;notice.textContent='Local file access cleared.';
 });
 document.querySelector('canvas').addEventListener('click',event=>{
  if(event.button!==0||!event.ctrlKey||event.altKey||event.shiftKey||event.metaKey||event.defaultPrevented)return;
  const frame=getFrame(),epoch=getEpoch();if(epoch===null||!frame||frame.epoch!==epoch||frame.session!=='pty-demo')return;
  const rect=event.currentTarget.getBoundingClientRect();
  if(!rect.width||!rect.height)return;
  const column=Math.floor((event.clientX-rect.left)/rect.width*frame.cols),row=Math.floor((event.clientY-rect.top)/rect.height*frame.rows.length);
  // Include unsupported names when detecting ambiguity, but never offer
  // them as preview candidates. Single-file mode has no terminal authority.
  if(!paths.length)return;
  const path=selectedPathAtCell(frame,row,column,preview.authorizedPaths());if(!path||!paths.includes(path))return;
  event.preventDefault();preview.dismiss();origin=epoch;choices.value=path;
  // Output cannot assert local/remote identity. Require an explicit local
  // confirmation before reading a same-named selected file, even after Ctrl-click.
  panel.hidden=false;title.textContent=path;body.textContent='';
  notice.textContent='Terminal reference selected. Verify it refers to this local folder, not a remote host. Use Preview selected file to confirm the local file; no bytes have been read.';
  document.querySelector('#preview-confirm-local').focus();
 });
 return preview;
}

