// Clipboard text is untrusted. Review multiline text before enqueueing it; Rust
// remains the authority for paste encoding and protocol state at delivery time.
// JSON alone leaves format characters (including bidi controls) invisible.
// Escape those code points as JSON UTF-16 units in the secondary review only;
// keep the readable preview and the original delivery payload untouched.
function quotedOriginal(text) {
  return JSON.stringify(text).replace(/\p{Cf}/gu, character =>
    character.split('').map(unit => '\\u'+unit.charCodeAt(0).toString(16).padStart(4,'0')).join(''));
}

export function installPasteReview({panel,preview,escaped,confirm,cancel,getEpoch,send,onError}) {
  let pending=null;
  function reset(){pending=null;preview.value='';escaped.textContent='';panel.hidden=true;confirm.disabled=true;}
  reset();
  function offer(text,returnFocus=null){
    // A new clipboard event supersedes an earlier review, including invalid data.
    reset();
    const epoch=getEpoch();if(epoch===null)return;
    // UTF-8 needs at least one byte per UTF-16 code unit, including lone
    // surrogates. Reject oversized strings before allocating an encoded copy.
    if(typeof text!=='string'||text.length>4096||new TextEncoder().encode(text).length>4096){onError('Paste exceeds 4096-byte limit; nothing sent.');return;}
    // Reject controls early without locking an otherwise healthy terminal.
    if(/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(text)){onError('Paste contains unsupported control characters; nothing sent.');return;}
    if(!text)return;
    if(!/[\r\n]/.test(text)){send(text,epoch);return;}
    // Textarea value normalizes CR/CRLF. Keep an escaped original separately,
    // using textContent (never HTML), and never send either rendered preview.
    // The original has already passed the 4096-byte guard above.
    pending={text,epoch,returnFocus};preview.value=text;escaped.textContent=quotedOriginal(text);panel.hidden=false;confirm.disabled=false;
  }
  function finish(deliver){
    const item=pending;
    // Only return focus that still belongs to this review. Lifecycle resets,
    // an explicit move elsewhere, and old sessions must never steal focus.
    const ownedFocus=panel.contains?.(panel.ownerDocument?.activeElement)===true;
    reset();
    if(!item||item.epoch!==getEpoch())return;
    if(deliver)send(item.text,item.epoch);
    if(ownedFocus&&item.epoch===getEpoch()&&item.returnFocus?.isConnected)
      item.returnFocus.focus();
  }
  confirm.addEventListener('click',()=>finish(true));
  cancel.addEventListener('click',()=>finish(false));
  panel.addEventListener('keydown',event=>{
    // Cancel only this review's plain Escape. Composition cancellation and
    // modified shortcuts belong to the input method or focused control.
    if(!pending||panel.hidden||event.key!=='Escape'||event.defaultPrevented||
      event.isComposing||event.keyCode===229||event.ctrlKey||event.altKey||
      event.shiftKey||event.metaKey||!panel.contains?.(event.target))return;
    event.preventDefault();event.stopPropagation();finish(false);
  });
  return {offer,reset};
}

