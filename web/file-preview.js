import {MAX_IMAGE_BYTES,rasterInfo,decodeRaster} from './raster-preview.js';
// Host-owned, opt-in local snapshot preview. No terminal output grants access:
// only File objects supplied by the user's directory picker may be read.
const MAX_FILES=2048, MAX_TEXT_BYTES=256*1024;
const TEXT_EXT=/\.(?:txt|md|markdown|tsx?|jsx?|json|css|rs|toml|ya?ml|svg|html?|xml|sh|py|c|h|cpp)(?::[0-9]+){0,2}$/i;
export function localSnapshotPath(path){
 if(typeof path!=='string'||path.length>4096||!path||/^[A-Za-z]:/.test(path)||path.startsWith('/')||path.includes('\\')||/[\p{Cc}\p{Cf}]/u.test(path))return null;
 const parts=path.split('/');
 // Only a numeric suffix on the final filename is a literal colon name.
 // Never admit schemes, drive letters or colon-bearing directories.
 if(parts.some((p,i)=>p.includes(':')&&(i!==parts.length-1||!/^[^:]+(?::[0-9]+){1,2}$/.test(p))))return null;
 return parts.every(p=>p&&p!=='.'&&p!=='..')?path:null;
}
const IMAGE_EXT=/\.(?:png|jpe?g)$/i;
export function createFilePreview({render,onError,decodeImage=decodeRaster}){
 let files=new Map(),root=null,generation=0,reading=false;
 function clear(){generation++;files=new Map();root=null;render(null);}
 function select(list){
  clear();
  if(!list||!Number.isInteger(list.length)||list.length>MAX_FILES){onError('Folder exceeds the 2048-file limit. Choose a smaller folder or use Select one source file without enumerating the project.');return [];}
  const next=new Map();let chosen=null;
  for(const file of list){
   const path=localSnapshotPath(file.webkitRelativePath),split=path?.indexOf('/')??-1;
   if(split<1){onError('Directory selection is unavailable or contains an unsafe path.');return [];}
   const name=path.slice(0,split),relative=path.slice(split+1);
   if((chosen!==null&&chosen!==name)||next.has(relative)){onError('Ambiguous directory selection; nothing is authorized.');return [];}
   chosen=name;next.set(relative,file);
  }
  files=next;root=chosen;return [...files.keys()].filter(p=>TEXT_EXT.test(p)||IMAGE_EXT.test(p)).sort();
 }
 // A single explicit file needs no directory enumeration. Its basename is
 // only a display key, never a terminal-relative path or filesystem authority.
 function selectFile(file){
  clear();
  if(!file||!localSnapshotPath(file.name)||file.name.includes('/')){
   onError('Select one source file with a safe basename.');return [];
  }
  root='Explicit single-file snapshot';files.set(file.name,file);
  return [file.name];
 }
 async function show(path){
  if(reading){onError('A preview read is still in progress. Retry when it finishes.');return;}
  const ticket=++generation;
  if(!localSnapshotPath(path)||!files.has(path)){render(null);onError('Not in the explicitly selected local folder snapshot.');return;}
  const file=files.get(path),owner=root;
  const image=IMAGE_EXT.test(path),limit=image?MAX_IMAGE_BYTES:MAX_TEXT_BYTES;
  if(!TEXT_EXT.test(path)&&!image){render(null);onError('Unsupported preview type.');return;}
  if(!Number.isSafeInteger(file.size)||file.size<0||file.size>limit){render(null);onError(image?'Image exceeds 2 MiB.':'Preview exceeds the 256 KiB text limit.');return;}
  reading=true;render({path,text:'Loading local source…',root:owner});
  try{
   const bytes=await file.arrayBuffer();
   if(ticket!==generation||owner!==root)return;
   if(bytes.byteLength>limit)throw Error(image?'Image exceeds 2 MiB.':'Preview exceeds the 256 KiB text limit.');
   if(image){
    const info=rasterInfo(bytes,path);
    const bitmap=await decodeImage(bytes,info);
    try{
     if(ticket!==generation||owner!==root)return;
     if(bitmap.width!==info.width||bitmap.height!==info.height)throw Error('Decoded image dimensions do not match the admitted header.');
     render({path,root:owner,bitmap,...info});
    }finally{bitmap.close();}
    return;
   }
   const text=new TextDecoder('utf-8',{fatal:true}).decode(bytes);
   if(text.includes('\0'))throw Error('Binary content is not a text preview.');
   // The renderer uses textContent, including for Markdown, TSX, HTML and SVG.
   render({path,text,root:owner});
  }catch(error){if(ticket===generation&&owner===root){render(null);onError(String(error?.message??error));}}
  finally{reading=false;}
 }
 function dismiss(){generation++;render(null);}
 return {select,selectFile,show,dismiss,clear,authorizedPaths:()=>[...files.keys()]};
}

// Match only exact names in the explicitly selected snapshot. This is not a
// generic path parser or OSC 8 resolver. Numeric location suffixes select the
// same authorized file, not a filesystem fallback or an editor navigation.
// Authorized literal numeric-colon filenames win over location interpretations. Wrapped paths still wait for trusted source identity.
export function selectedPathAtCell(frame,row,column,paths){
 if(!frame||!Number.isInteger(row)||!Number.isInteger(column)||row<0||column<0||column>=frame.cols||!frame.rows[row])return null;
 let text='',at=-1;
 frame.rows[row].forEach((cell,col)=>{if(cell.width===0){if(col===column)at=Math.max(0,text.length-1);return;}if(col===column)at=text.length;text+=cell.text;});
 if(at<0)return null;
 const boundary=char=>char===undefined||/[\s`'"()[\]{}「」『』“”‘’《》]/u.test(char);
 const matches=[];
 for(const path of paths){
  if(!localSnapshotPath(path))continue;
  for(const token of [path,'./'+path]){
   for(let start=text.indexOf(token);start!==-1;start=text.indexOf(token,start+1)){
    let end=start+token.length,hasLocation=false;
    if(text[end]===':'){
     // Keep location parsing bounded and reject partial matches such as
     // :0, :12:bad, :12:3:4, huge integers, or URI-like suffixes.
     const location=/^:([1-9][0-9]{0,9})(?::([1-9][0-9]{0,9}))?/.exec(text.slice(end,end+22));
     if(!location||Number(location[1])>4294967295||(location[2]&&Number(location[2])>4294967295))continue;
     end+=location[0].length;hasLocation=true;
    }
    // A punctuation suffix is prose only when the whole run reaches a
    // boundary. Never peel a prefix from README.md.js or README.md,backup.
    let contextEnd=end;
    while(contextEnd<text.length&&/[.,;!?，。；！？、]/u.test(text[contextEnd]))contextEnd++;
    if(at>=start&&at<contextEnd&&boundary(text[start-1])&&boundary(text[contextEnd]))
     matches.push({path,end,hasLocation,start,contextEnd});

   }
  }
 }
 // Literal authorized names may themselves include punctuation. Reject
 // competing interpretations even when clicking their punctuation, rather
 // than choosing the shorter prose interpretation or longest filename.
 const candidates=matches.filter(m=>!m.hasLocation||!matches.some(exact=>!exact.hasLocation&&exact.start===m.start&&exact.end===m.end));
 return new Set(candidates.map(m=>m.path)).size===1&&candidates.some(m=>at<m.end)?candidates[0].path:null;
}
