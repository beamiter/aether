// Desktop composition ordering adapter. Only committed text leaves this module.
// Native WebKit/platform IME behavior still requires real-window acceptance.
export function installIme(element,{getEpoch,commit}){
 let active=false,owner=null,duplicate=null,discard=false;
 const clear=()=>{element.value='';};
 const reset=()=>{active=false;owner=null;duplicate=null;discard=true;clear();};
 element.addEventListener('compositionstart',()=>{active=true;owner=getEpoch();duplicate=null;discard=false;});
 element.addEventListener('compositionend',event=>{
   if(!active){clear();return;}
   active=false;const epoch=owner;owner=null;const text=event.data||'';
   // The typing channel cannot accept more than 4096 UTF-8 bytes. Do not
   // retain an unbounded rejected composition just to recognize its duplicate.
   // A boolean quarantine drops late input until a new key/composition/reset.
   if(text.length>4096){duplicate=null;discard=true;}else duplicate=text;
   clear();
   if(epoch!==null&&epoch===getEpoch()&&text)commit(text,epoch);
   else discard=true;
 });
 element.addEventListener('keydown',event=>{
   // Some IMEs report keyCode 229 without isComposing or a Process key.
   // It is not a new ordinary key and must not release late-input guards.
   if(!active&&!event.isComposing&&event.keyCode!==229&&event.key!=='Process'&&event.key!=='Dead'){duplicate=null;discard=false;}
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
   const epoch=getEpoch();if(epoch!==null&&text)commit(text,epoch);
 });
 element.addEventListener('blur',reset);
 return {reset,isComposing:()=>active};
}

