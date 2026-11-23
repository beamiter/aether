// Desktop composition ordering adapter. Preedit stays in the view.
export function installIme(element,{getEpoch,commit,preedit=()=>{},afterEnd=callback=>callback(),canCommit=()=>true}){
 let active=false,owner=null,duplicate=null,discard=false,pending=null;
 const clear=()=>{element.value='';};
 const flush=()=>{
   const completed=pending;pending=null;
   if(completed&&completed.epoch===getEpoch()&&canCommit())commit(completed.text,completed.epoch);
 };
 const reset=()=>{active=false;owner=null;duplicate=null;discard=true;pending=null;clear();preedit('');};
 element.addEventListener('compositionstart',()=>{
   flush();active=true;owner=getEpoch();duplicate=null;discard=false;preedit('');
 });
 element.addEventListener('compositionupdate',event=>{
   const text=event.data??'';
   preedit(active&&owner!==null&&owner===getEpoch()&&text.length<=4096?text:'');
 });
 element.addEventListener('compositionend',event=>{
   if(!active){clear();return;}
   active=false;const epoch=owner;owner=null;const text=event.data||'';
   // The typing channel cannot accept more than 4096 UTF-8 bytes. Do not
   // retain an unbounded rejected composition just to recognize its duplicate.
   // A boolean quarantine drops late input until a new key/composition/reset.
   if(text.length>4096){duplicate=null;discard=true;}else duplicate=text;
   clear();preedit('');
   if(epoch===null||epoch!==getEpoch()||!text){discard=true;return;}
   if(text.length>4096){commit(text,epoch);return;}
   const completed={text,epoch};pending=completed;
   // A blur-generated end can precede the browser's focus events. The
   // caller waits for focus to settle, while reset can revoke this candidate.
   afterEnd(()=>{if(pending===completed)flush();});
 });
 element.addEventListener('keydown',event=>{
   // A consumed shortcut is not fresh typing and cannot release late input.
   if(event.defaultPrevented)return;
   // Some IMEs report keyCode 229 without isComposing or a Process key.
   // It is not a new ordinary key and must not release late-input guards.
   if(!active&&!event.isComposing&&event.keyCode!==229&&event.key!=='Process'&&event.key!=='Dead'){
     flush();duplicate=null;discard=false;
   }
 });
 element.addEventListener('input',event=>{
   if(active||event.isComposing)return;
   const text=event.data??element.value;clear();
   if(discard)return;
   // Clipboard insertion belongs to paste review, never this typing channel.
   // Drop, replacement, undo/redo and unknown browser edits are not keystrokes.
   // Composition commits are delivered exclusively by compositionend above.
   if(event.inputType!=='insertText')return;
   if(duplicate!==null){const previous=duplicate;duplicate=null;if(text===previous)return;}
   flush();
   const epoch=getEpoch();if(epoch!==null&&text)commit(text,epoch);
 });
 element.addEventListener('blur',reset);
 return {reset,isComposing:()=>active,isSettled:()=>!active&&pending===null};
}

