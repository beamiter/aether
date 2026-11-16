// Clipboard text is untrusted. Review multiline text before enqueueing it; Rust
// remains the authority for paste encoding and protocol state at delivery time.
export function installPasteReview({panel,preview,confirm,cancel,getEpoch,send,onError}) {
  let pending=null;
  function reset(){pending=null;preview.value='';panel.hidden=true;confirm.disabled=true;}
  reset();
  function offer(text){
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
    pending={text,epoch};preview.value=text;panel.hidden=false;confirm.disabled=false;
  }
  confirm.addEventListener('click',()=>{
    const item=pending;reset();
    if(!item||item.epoch!==getEpoch())return;
    send(item.text,item.epoch);
  });
  cancel.addEventListener('click',reset);
  return {offer,reset};
}
