// Desktop composition ordering adapter. Only committed text leaves this module.
// Native WebKit/platform IME behavior still requires real-window acceptance.
export function installIme(element,{getEpoch,commit}){
 let active=false,owner=null,duplicate=null,discard=false;
 const clear=()=>{element.value='';};
 const reset=()=>{active=false;owner=null;duplicate=null;discard=true;clear();};
 element.addEventListener('compositionstart',()=>{active=true;owner=getEpoch();duplicate=null;discard=false;});
 element.addEventListener('compositionend',event=>{
   if(!active){clear();return;}
   active=false;const epoch=owner;owner=null;const text=event.data||'';duplicate=text;clear();
   if(epoch!==null&&epoch===getEpoch()&&text)commit(text,epoch);
   else discard=true;
 });
 element.addEventListener('keydown',event=>{
   if(!active&&!event.isComposing&&event.key!=='Process'&&event.key!=='Dead'){duplicate=null;discard=false;}
 });
 element.addEventListener('input',event=>{
   if(active||event.isComposing)return;
   const text=event.data??element.value;clear();
   if(discard)return;
   if(duplicate!==null){const previous=duplicate;duplicate=null;if(text===previous)return;}
   if(event.inputType==='insertCompositionText'||event.inputType==='insertFromComposition')return;
   const epoch=getEpoch();if(epoch!==null&&text)commit(text,epoch);
 });
 element.addEventListener('blur',reset);
 return {reset,isComposing:()=>active};
}
