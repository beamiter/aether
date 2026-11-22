import {createFilePreview,selectedReferenceAtCell} from './file-preview.js';
import {terminalCellAt} from './viewport.js';
// This panel is explicitly local and independent of terminal/Codex output.
// Directory selection grants a snapshot, not arbitrary path or remote access.
export function installFilePreviewPanel(document,{getFrame=()=>null,getEpoch=()=>null,getSession=()=>'pty-demo',
 getOwner=()=>getEpoch()===null?null:JSON.stringify([getSession(),getEpoch()]),isLocalTerminal=()=>true,openInVim=null}={}){
 let paths=[],origin=null,selectionError=false,location=null,snapshot=null,opening=false;
 const picker=document.querySelector('#preview-folder'),choices=document.querySelector('#preview-files'),panel=document.querySelector('#file-preview'),title=document.querySelector('#preview-title'),body=document.querySelector('#preview-source'),notice=document.querySelector('#preview-notice');
 const raster=document.querySelector('#preview-raster');
 const confirmation=document.querySelector('#preview-confirmation'),confirm=document.querySelector('#preview-confirm-local'),showButton=document.querySelector('#preview-show');
 const edit=document.querySelector('#preview-vim9');
 function editControl(){
  edit.hidden=!snapshot||!openInVim||(origin!==null&&!isLocalTerminal());
  edit.disabled=edit.hidden||opening;
 }
 confirmation.hidden=true;
 const preview=createFilePreview({render(item,source=null){
  snapshot=source;editControl();
  const active=document.activeElement;
  const moveFocus=(!confirmation.hidden&&active===confirm)||(!item&&active===body);
  confirmation.hidden=true;
  raster.width=1;raster.height=1;raster.hidden=true;
  if(item?.bitmap){raster.width=item.width;raster.height=item.height;const context=raster.getContext('2d');if(!context)throw Error('Image canvas unavailable.');context.drawImage(item.bitmap,0,0);raster.hidden=false;}
  panel.hidden=!item;title.textContent=item?item.path:'';body.textContent=item?.bitmap?'Local image preview':item?item.text:'';
  if(moveFocus)(item?body:showButton).focus();
 },onError(message){selectionError=true;notice.textContent=message;}});
 const single=document.querySelector('#preview-single');
 function populate(selected){
  origin=null;location=null;choices.replaceChildren();
  for(const path of selected){const option=document.createElement('option');option.value=path;option.textContent=path;choices.append(option);}
  choices.disabled=!selected.length;showButton.disabled=!selected.length;
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
 choices.addEventListener('change',()=>{origin=null;location=null;preview.dismiss();});
 async function show(){if(origin!==null&&origin!==getOwner()){preview.dismiss();notice.textContent='Terminal session changed. Select a local file explicitly again.';return;}notice.textContent='';await preview.show(choices.value);}
 showButton.addEventListener('click',show);
 confirm.addEventListener('click',()=>{if(!confirmation.hidden)return show();});
 function closeFromPanel(fromEscape=false){
  if(panel.hidden)return;
  preview.dismiss();notice.textContent='Preview closed. No terminal input sent.';
  // Escape stays away from the terminal so held-key repeat cannot send PTY
  // input. Explicit Close may restore only the same terminal session.
  const target=!fromEscape&&origin!==null&&origin===getOwner()?document.querySelector('canvas'):showButton;
  target.focus();
 }
 document.querySelector('#preview-close').addEventListener('click',()=>closeFromPanel());
 edit.addEventListener('click',async()=>{
  const source=snapshot, owner=origin, position=location;
  const valid=()=>source===snapshot&&(owner===null||(owner===getOwner()&&isLocalTerminal()));
  if(!source||!openInVim||opening||!valid())return;
  opening=true;editControl();
  try{
   const digest=await globalThis.crypto.subtle.digest('SHA-256',source.bytes);
   if(!valid())return;
   const sha256=Array.from(new Uint8Array(digest),byte=>byte.toString(16).padStart(2,'0')).join('');
   const reference={source:'local_snapshot',directory:source.directory,path:source.path,version:{bytes:source.bytes.byteLength,sha256}};
   const opened=await openInVim(reference,position?.line??null,position?.column??null);
   if(valid())notice.textContent=opened?'Opened the verified local file in Vim9.':'File selection cancelled.';
  }catch(error){if(valid())notice.textContent=String(error?.message??error);}
  finally{opening=false;editControl();}
 });
 panel.addEventListener('keydown',event=>{
  if(panel.hidden||!panel.contains(document.activeElement)||event.key!=='Escape'||event.defaultPrevented||event.isComposing||event.keyCode===229||event.ctrlKey||event.altKey||event.shiftKey||event.metaKey||event.getModifierState?.('AltGraph'))return;
  event.preventDefault();event.stopPropagation();closeFromPanel(true);
 });
 document.querySelector('#preview-clear').addEventListener('click',()=>{
  preview.clear();paths=[];origin=null;picker.value='';single.value='';choices.replaceChildren();choices.disabled=true;showButton.disabled=true;notice.textContent='Local file access cleared.';
 });
 document.querySelector('canvas').addEventListener('click',event=>{
  if(event.button!==0||!event.ctrlKey||event.altKey||event.shiftKey||event.metaKey||event.defaultPrevented)return;
  const frame=getFrame(),epoch=getEpoch();if(epoch===null||!frame||frame.epoch!==epoch||frame.session!==getSession())return;
  const point=terminalCellAt(frame,event.currentTarget,event.clientX,event.clientY);
  if(!point)return;
  // Include unsupported names when detecting ambiguity, but never offer
  // them as preview candidates. Single-file mode has no terminal authority.
  if(!paths.length)return;
  const reference=selectedReferenceAtCell(frame,point.row,point.col,preview.authorizedPaths()),path=reference?.path,owner=getOwner();
  if(!path||!paths.includes(path)||owner===null)return;
  event.preventDefault();preview.dismiss();origin=owner;location=reference;choices.value=path;
  // Output cannot assert local/remote identity. Require an explicit local
  // confirmation before reading a same-named selected file, even after Ctrl-click.
  panel.hidden=false;title.textContent=path;body.textContent='';
  notice.textContent='Terminal reference selected. Verify it refers to this local folder, not a remote host. Use Preview selected file to confirm the local file; no bytes have been read.';
  confirmation.hidden=false;confirm.focus();
 });
 return preview;
}

