import {installBlockSearch} from './command-block-search.js';

const MAX_BLOCKS=8;
const MAX_OUTPUT_BYTES=16*1024;
const encoder=new TextEncoder();
const decimal=value=>typeof value==='string'&&/^[0-9]{1,20}$/.test(value);
const bounded=(value,limit)=>typeof value==='string'&&value.length<=limit&&encoder.encode(value).length<=limit;
const sha256=value=>typeof value==='string'&&/^[0-9a-f]{64}$/.test(value);
const streamId=value=>typeof value==='string'&&/^[0-9a-f]{32}$/.test(value);
const uuid4=value=>typeof value==='string'&&/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(value);
const sameOwner=(left,right)=>left?.session===right.session&&left?.epoch===right.epoch&&left?.generation===right.generation;

function validOutputSource(source){
  if(!source||![source.byte_start,source.byte_end,source.start_event,source.end_event].every(
      value=>decimal(value)&&BigInt(value)<=0xffffffffffffffffn))return false;
  if((source.stream_id!==undefined||source.execution_id!==undefined)&&
      (!streamId(source.stream_id)||!sha256(source.execution_id)))return false;
  const size=value=>Array.isArray(value)&&value.length===2&&value.every(
      dimension=>Number.isInteger(dimension)&&dimension>0&&dimension<=65535);
  return BigInt(source.byte_end)>=BigInt(source.byte_start)&&BigInt(source.start_event)>0n&&
      BigInt(source.end_event)>BigInt(source.start_event)&&size(source.start_size)&&size(source.end_size)&&
      typeof source.resized==='boolean'&&
      (source.resized||source.start_size.every((dimension,index)=>dimension===source.end_size[index]));
}

export function validateCommandBlockSnapshot(snapshot,owner){
  const identity=snapshot?.owner;
  if(!sameOwner(identity,owner)||typeof snapshot.enabled!=='boolean'||
      !Array.isArray(snapshot.blocks)||snapshot.blocks.length>MAX_BLOCKS||
      !Array.isArray(snapshot.bookmarked)||snapshot.bookmarked.length>snapshot.blocks.length||
      (!snapshot.enabled&&snapshot.blocks.length)){
    throw new Error('Invalid command block snapshot.');
  }
  let previous=null;
  for(const block of snapshot.blocks){
    const output=block?.output;
    if(!decimal(block?.sequence)||BigInt(block.sequence)===0n||
        (previous!==null&&BigInt(block.sequence)>=previous)||
        !sha256(block.revision)||block.provenance!=='shell_reported'||!bounded(block.execution_id,1024)||!block.execution_id||
        !bounded(block.command,16*1024)||!block.command.trim()||
        (block.cwd!==null&&!bounded(block.cwd,4096))||
        (block.shell_session!==null&&!bounded(block.shell_session,128))||
        (block.shell_sequence!==null&&!decimal(block.shell_sequence))||
        (block.exit!==null&&(!Number.isInteger(block.exit)||block.exit< -2147483648||block.exit>2147483647))||
        (block.output_source!==undefined&&!validOutputSource(block.output_source))||
        !['text','screen_operation','byte_limit'].includes(output?.kind)||
        (output.kind==='text'&&(!bounded(output.text,MAX_OUTPUT_BYTES)||/[\x00-\x08\x0b-\x1f\x7f-\x9f]/.test(output.text)))){
      throw new Error('Invalid command block record.');
    }
    previous=BigInt(block.sequence);
  }
  if(new Set(snapshot.bookmarked).size!==snapshot.bookmarked.length||
      snapshot.bookmarked.some(sequence=>!snapshot.blocks.some(block=>block.sequence===sequence))){
    throw new Error('Invalid command block bookmarks.');
  }
}

export function validateFrozenReview(receipt,owner){
  if(!sameOwner(receipt?.owner,owner)||!uuid4(receipt.reviewId)||typeof receipt.complete!=='boolean'||
      !bounded(receipt.summary,1024)||!bounded(receipt.comparison,512*1024)){
    throw Error('Invalid frozen output review receipt.');
  }
}

export function installCommandBlocks(document,{getOwner,read,bookmark,find,compare,exportReview,recall,returnToTerminal}){
  const controls=document.querySelector('#command-blocks');
  const toggle=document.querySelector('#command-block-toggle');
  const panel=document.querySelector('#command-block-panel');
  const refresh=document.querySelector('#command-block-refresh');
  const status=document.querySelector('#command-block-status');
  const list=document.querySelector('#command-block-list');
  const bookmarkFilter=document.querySelector('#command-block-bookmarked');
  const noBookmarks=document.querySelector('#command-block-no-bookmarks');
  const reviewForm=document.querySelector('#command-review-form');
  const beforeSelect=document.querySelector('#command-review-before');
  const afterSelect=document.querySelector('#command-review-after');
  const compareButton=document.querySelector('#command-review-compare');
  const reviewPanel=document.querySelector('#command-review');
  const reviewStatus=document.querySelector('#command-review-status');
  const reviewMarkdown=document.querySelector('#command-review-markdown');
  const reviewJson=document.querySelector('#command-review-json');
  const cards=new Map();
  const search=installBlockSearch(document,{
    getOwner:()=>controls.hidden||panel.hidden?null:getOwner(),
    scan:find,
    cardFor:record=>{
      const card=cards.get(record.sequence);
      if(card?.dataset.revision!==record.revision)return null;
      if(card.hidden){bookmarkedOnly=false;applyBookmarkFilter();}
      return card;
    },
  });
  let owner=null,request=null;
  let bookmarkedOnly=false,bookmarkOperation=null;
  let chat=false,chatVisible=false,manuallyOpen=false;
  let desiredSequence=null,displayedSequence,attemptedSequence;
  let choices=new Map(),frozen=null,reviewOperation=null;

  function setOpen(open){
    panel.hidden=!open;
    toggle.setAttribute('aria-expanded',String(open));
    if(!open){request=null;attemptedSequence=undefined;refresh.disabled=!!bookmarkOperation;search.reset();}
  }

  function updateOwner(){
    const next=getOwner();
    controls.hidden=!next||(chat&&!chatVisible);
    if(next===owner)return;
    owner=next;
    manuallyOpen=false;
    bookmarkOperation=null;
    bookmarkedOnly=false;
    bookmarkFilter.hidden=true;
    setOpen(false);
    desiredSequence=null;
    displayedSequence=attemptedSequence=undefined;
    status.textContent='';
    cards.clear();
    list.replaceChildren();
    applyBookmarkFilter();
    search.setAvailable(false);
    reviewOperation=null;
    frozen=null;
    reviewPanel.hidden=true;
    delete reviewPanel.dataset.reviewId;
    document.querySelector('#command-review-diff').textContent='';
    document.querySelector('#command-review-summary').textContent='';
    reviewStatus.textContent='';
    setChoices([]);
  }

  function setChatPresentation(enabled,visible,newestCompleted){
    chat=enabled;
    chatVisible=visible;
    desiredSequence=newestCompleted;
    toggle.hidden=chat;
    const hide=!owner||(chat&&!chatVisible);
    if(hide&&!controls.hidden)search.reset();
    controls.hidden=hide;
    const open=chat||manuallyOpen;
    if(panel.hidden===open)setOpen(open);
    refreshChat();
  }

  function refreshChat(){
    if(!chat||controls.hidden||request||bookmarkOperation||attemptedSequence===desiredSequence)return;
    if(displayedSequence!==undefined&&(desiredSequence===null||
        (displayedSequence!==null&&BigInt(displayedSequence)>=BigInt(desiredSequence))))return;
    attemptedSequence=desiredSequence;
    void refreshList();
  }

  function element(type,text,css){
    const node=document.createElement(type);
    if(text!==undefined)node.textContent=text;
    if(css)node.className=css;
    return node;
  }

  function reviewControls(){
    const usable=choices.size>=2;
    reviewForm.hidden=!usable;
    beforeSelect.disabled=!usable||!!reviewOperation;
    afterSelect.disabled=beforeSelect.disabled;
    compareButton.disabled=beforeSelect.disabled||beforeSelect.value===afterSelect.value;
    reviewMarkdown.disabled=!!reviewOperation;
    reviewJson.disabled=reviewMarkdown.disabled;
  }

  function setChoices(blocks){
    choices=new Map(blocks.filter(block=>block.output.kind==='text').map(block=>[block.revision,block]));
    const keys=[...choices.keys()];
    const previous=[beforeSelect.value,afterSelect.value];
    for(const [index,select] of [beforeSelect,afterSelect].entries()){
      select.replaceChildren();
      for(const [key,block] of choices){
        const command=block.command.replace(/[\r\n\t]/g,' ');
        const option=element('option',`#${block.sequence} · ${command.slice(0,110)}${command.length>110?'…':''}`);
        option.value=key;
        select.append(option);
      }
      select.value=choices.has(previous[index])?previous[index]:keys[index===0?1:0]??'';
    }
    reviewControls();
  }

  function selection(select){
    const block=choices.get(select.value);
    return block?{sequence:block.sequence,revision:block.revision}:null;
  }

  async function freezeSelection(event){
    event.preventDefault();
    const attachment=getOwner(),before=selection(beforeSelect),after=selection(afterSelect);
    if(!attachment||!before||!after||before.sequence===after.sequence||reviewOperation)return;
    const action={};
    reviewOperation=action;
    reviewControls();
    try{
      const receipt=await compare(attachment,before,after);
      if(getOwner()!==attachment||reviewOperation!==action||panel.hidden)return;
      validateFrozenReview(receipt,attachment);
      frozen=receipt;
      reviewPanel.hidden=false;
      reviewPanel.dataset.reviewId=receipt.reviewId;
      document.querySelector('#command-review-summary').textContent=receipt.summary;
      document.querySelector('#command-review-diff').textContent=receipt.comparison;
      reviewStatus.textContent='';
    }catch(error){
      if(reviewOperation===action&&getOwner()===attachment&&!panel.hidden)reviewStatus.textContent=String(error);
    }finally{
      if(reviewOperation===action){reviewOperation=null;reviewControls();}
    }
  }

  async function saveReview(format,extension){
    const review=frozen;
    if(!review||reviewOperation)return;
    const action={};
    reviewOperation=action;
    reviewControls();
    try{
      const file=await exportReview(review.reviewId,format);
      if(reviewOperation!==action||frozen!==review||panel.hidden)return;
      if(file.filename!==`aether-output-${review.reviewId}.${extension}`||!bounded(file.content,2*1024*1024)){
        throw Error('Invalid output review export.');
      }
      const link=element('a');
      const url=URL.createObjectURL(new Blob([file.content],{type:format==='json'?'application/json':'text/markdown'}));
      link.href=url;
      link.download=file.filename;
      document.body.append(link);
      try{link.click();}finally{link.remove();setTimeout(()=>URL.revokeObjectURL(url),5000);}
      reviewStatus.textContent=`Download requested for ${file.filename}.`;
    }catch(error){
      if(reviewOperation===action&&frozen===review&&!panel.hidden)reviewStatus.textContent=String(error);
    }finally{
      if(reviewOperation===action){reviewOperation=null;reviewControls();}
    }
  }

  function applyBookmarkFilter(){
    bookmarkFilter.setAttribute('aria-pressed',String(bookmarkedOnly));
    let anyVisible=false;
    for(const card of cards.values()){
      card.hidden=bookmarkedOnly&&card.dataset.bookmarked!=='true';
      anyVisible ||= !card.hidden;
    }
    noBookmarks.hidden=!bookmarkedOnly||anyVisible;
  }

  function showBookmark(card,bookmarked){
    card.dataset.bookmarked=String(bookmarked);
    card.querySelector('.command-block-bookmark').setAttribute('aria-pressed',String(bookmarked));
  }

  async function changeBookmark(card,button,attachment){
    if(getOwner()!==attachment||panel.hidden||controls.hidden||request||bookmarkOperation)return;
    const {sequence,revision}=card.dataset;
    const operation={};
    const bookmarked=card.dataset.bookmarked!=='true';
    bookmarkOperation=operation;
    refresh.disabled=true;
    try{
      await bookmark(attachment,{sequence,revision},bookmarked);
      if(bookmarkOperation!==operation||getOwner()!==attachment)return;
      showBookmark(card,bookmarked);
      if(bookmarkedOnly&&!bookmarked&&document.activeElement===button)bookmarkFilter.focus();
      applyBookmarkFilter();
      if(!panel.hidden)status.textContent=bookmarked?'Command bookmarked.':'Bookmark removed.';
    }catch(error){
      if(bookmarkOperation===operation&&getOwner()===attachment&&!panel.hidden){
        status.textContent=String(error?.message??error);
      }
    }finally{
      if(bookmarkOperation===operation){bookmarkOperation=null;refresh.disabled=false;refreshChat();}
    }
  }

  function makeCard(block,attachment){
    const card=element('details',undefined,'command-block');
    card.dataset.sequence=block.sequence;
    card.dataset.revision=block.revision;
    const actions=element('div',undefined,'command-block-actions');
    const summary=element('summary');
    const command=element('code',block.command,'command-block-command');
    const outcome=block.exit===null?'Exit status unavailable':`Exit ${block.exit}`;
    summary.append(command,element('span',outcome,'command-block-outcome'));
    card.append(summary);
    const provenance=element('p',`PTY-reported OSC 133 · ${block.execution_id}`,'command-block-meta');
    card.append(provenance);
    if(block.output_source){
      const source=block.output_source;
      const start=source.start_size.join('×');
      const grid=source.resized?`${start} to ${source.end_size.join('×')}`:start;
      const metadata=element('p',`PTY bytes [${source.byte_start}, ${source.byte_end}) · marker events ${source.start_event}–${source.end_event} · grid ${grid}`,
        'command-block-meta command-block-output-source');
      if(source.stream_id)metadata.title=`PTY stream ${source.stream_id}\nExecution ${source.execution_id}`;
      card.append(metadata);
    }
    if(block.cwd)card.append(element('p',block.cwd,'command-block-meta'));
    if(block.output.kind!=='text'){
      card.append(element('p',block.output.kind==='byte_limit'
        ?'Output exceeds the command block size limit.'
        :'The command output used live terminal screen operations.'));
    }else if(!block.output.text){
      card.append(element('p','No output.'));
    }else{
      const text=element('textarea');
      text.value=block.output.text;
      text.readOnly=true;
      text.rows=Math.min(10,Math.max(2,block.output.text.split('\n').length));
      text.setAttribute('aria-label','Command output');
      text.spellcheck=false;
      const copy=element('button','Copy output');
      copy.type='button';
      copy.addEventListener('click',async()=>{
        if(getOwner()!==attachment)return;
        try{
          if(!globalThis.navigator?.clipboard?.writeText)throw new Error('Clipboard unavailable');
          await globalThis.navigator.clipboard.writeText(text.value);
          if(getOwner()===attachment&&!panel.hidden)status.textContent='Output copied.';
        }catch{
          if(getOwner()!==attachment||panel.hidden||!text.isConnected)return;
          text.focus();text.select();
          let copied=false;
          try{copied=!!document.execCommand?.('copy');}catch{/* Selection stays available. */}
          status.textContent=copied
            ?'Output copied.'
            :'Output selected. Use your system copy shortcut.';
        }
      });
      card.append(text);
      actions.append(copy);
    }
    if(recall){
      const insert=element('button','Insert command at shell prompt');
      insert.type='button';insert.className='command-block-recall';
      insert.addEventListener('click',()=>{
        if(getOwner()!==attachment||panel.hidden)return;
        recall(block,attachment,insert,message=>{if(getOwner()===attachment)status.textContent=message;});
      });
      actions.append(insert);
    }
    const pin=element('button','Bookmark','command-block-bookmark');
    pin.type='button';
    pin.addEventListener('click',()=>void changeBookmark(card,pin,attachment));
    actions.append(pin);
    card.append(actions);
    return card;
  }

  async function refreshList(){
    const attachment=getOwner();
    if(!attachment||panel.hidden||request||bookmarkOperation)return;
    const reading={};
    request=reading;
    refresh.disabled=true;
    try{
      const snapshot=await read(attachment);
      if(request!==reading||getOwner()!==attachment)return;
      validateCommandBlockSnapshot(snapshot,attachment);
      setChoices(snapshot.blocks);
      search.setAvailable(snapshot.blocks.length>0);
      const retained=new Map(snapshot.blocks.map(block=>[block.sequence,block.revision]));
      const bookmarked=new Set(snapshot.bookmarked);
      let anchor,anchorTop;
      if(chat&&list.scrollTop>0){
        const top=list.getBoundingClientRect().top;
        anchor=[...list.children].find(card=>!card.hidden&&retained.get(card.dataset.sequence)===card.dataset.revision&&
          card.getBoundingClientRect().bottom>top);
        anchorTop=anchor?.getBoundingClientRect().top;
      }
      for(const [sequence,card] of cards){
        if(retained.get(sequence)!==card.dataset.revision){card.remove();cards.delete(sequence);}
      }
      for(const block of [...snapshot.blocks].reverse()){
        let card=cards.get(block.sequence);
        if(!card){card=makeCard(block,attachment);cards.set(block.sequence,card);list.prepend(card);}
        showBookmark(card,bookmarked.has(block.sequence));
      }
      bookmarkFilter.hidden=!snapshot.enabled;
      applyBookmarkFilter();
      if(anchor&&!anchor.hidden)list.scrollTop+=anchor.getBoundingClientRect().top-anchorTop;
      displayedSequence=snapshot.blocks[0]?.sequence??null;
      status.textContent=!snapshot.enabled
        ?'Command blocks are available for local sessions opened with jsh.'
        :snapshot.blocks.length
          ?'Showing completed commands reported by this local PTY.'
          :'Waiting for a completed command reported by this local jsh.';
    }catch(error){
      if(request===reading&&getOwner()===attachment)status.textContent=String(error?.message??error);
    }finally{
      if(request===reading){request=null;refresh.disabled=false;refreshChat();}
    }
  }

  toggle.addEventListener('click',()=>{
    if(!getOwner()||chat)return;
    manuallyOpen=panel.hidden;
    setOpen(manuallyOpen);
    if(!panel.hidden)void refreshList();
  });
  refresh.addEventListener('click',refreshList);
  bookmarkFilter.addEventListener('click',()=>{bookmarkedOnly=!bookmarkedOnly;applyBookmarkFilter();});
  reviewForm.addEventListener('submit',freezeSelection);
  beforeSelect.addEventListener('change',reviewControls);
  afterSelect.addEventListener('change',reviewControls);
  reviewMarkdown.addEventListener('click',()=>void saveReview('markdown','md'));
  reviewJson.addEventListener('click',()=>void saveReview('json','json'));
  document.querySelector('#command-block-return').addEventListener('click',()=>{
    if(!chat){manuallyOpen=false;setOpen(false);}
    returnToTerminal();
  });
  return {updateOwner,setChatPresentation};
}
