const uint = x => Number.isInteger(x) && x >= 1 && x <= 0xffffffff;
export function acceptFull(previous, incoming, expected = { session: 'frame-demo', epoch: 1 }) {
  const f = incoming;
  if (!f || f.version !== 3 || f.session !== expected.session || f.epoch !== expected.epoch ||
      !uint(f.revision) || !uint(f.layout)) throw new Error('foreign or invalid frame');
  if (previous && (f.revision <= previous.revision || f.layout < previous.layout)) throw new Error('stale frame');
  if (!Number.isInteger(f.cols) || f.cols < 1 || f.cols > 256 || !Array.isArray(f.rows) ||
      f.rows.length < 1 || f.rows.length > 128 || f.cols * f.rows.length > 32768) throw new Error('dimensions');
  if (previous && f.layout === previous.layout && (f.cols !== previous.cols || f.rows.length !== previous.rows.length)) throw new Error('layout must advance');
  if (typeof f.cursor_visible !== 'boolean' || typeof f.pending_wrap !== 'boolean' || !['block','underline','beam'].includes(f.cursor_shape)) throw new Error('cursor visibility');
  if (!Number.isInteger(f.cursor_row) || !Number.isInteger(f.cursor_col) || f.cursor_row < 0 || f.cursor_row >= f.rows.length || f.cursor_col < 0 || f.cursor_col >= f.cols) throw new Error('cursor');
  const rows = f.rows.map(row => {
    if (!Array.isArray(row) || row.length !== f.cols) throw new Error('row bounds');
    return Object.freeze(row.map(c => {
      if (!c || !Number.isInteger(c.width) || c.width<0 || c.width>2 || !Number.isInteger(c.style) || c.style<0 || c.style>31 || typeof c.text !== 'string' || [...c.text].length !== 1 || /[\x00-\x1f\x7f-\x9f\ud800-\udfff]/u.test(c.text) ||
        !/^#[a-f0-9]{6}$/i.test(c.foreground) || !/^#[a-f0-9]{6}$/i.test(c.background)) throw new Error('cell');
      return Object.freeze({text:c.text,foreground:c.foreground,background:c.background,width:c.width,style:c.style});
    }));
  });
  for (const row of rows) for (let i=0;i<row.length;i++) {
    if(row[i].width===2 && (i+1===row.length || row[i+1].width!==0))throw new Error('wide lead without continuation');
    if(row[i].width===0 && (i===0 || row[i-1].width!==2 || row[i].text!==' '))throw new Error('orphan wide continuation');
  }
  return Object.freeze({version:3,session:f.session,epoch:f.epoch,revision:f.revision,layout:f.layout,cols:f.cols,
    rows:Object.freeze(rows),cursor_row:f.cursor_row,cursor_col:f.cursor_col,cursor_visible:f.cursor_visible,cursor_shape:f.cursor_shape,pending_wrap:f.pending_wrap});
}
export function paint(canvas, frame, scale = globalThis.devicePixelRatio || 1) {
  if (!Number.isFinite(scale) || scale <= 0 || scale > 4) scale = 1;
  const cw = 12, ch = 24;
  // Cap backing allocation independently of viewport dimensions and device DPR.
  scale = Math.min(scale, Math.sqrt(16777216 / (frame.cols*cw*frame.rows.length*ch)));
  canvas.width = Math.max(1,Math.floor(frame.cols * cw * scale)); canvas.height = Math.max(1,Math.floor(frame.rows.length * ch * scale));
  canvas.style.width = `${frame.cols*cw}px`; canvas.style.height = `${frame.rows.length*ch}px`;
  const g = canvas.getContext('2d'); g.scale(scale,scale); g.font = '18px monospace'; g.textBaseline = 'top';
  // All backgrounds first: a wide glyph must not be erased by its trailing cell.
  frame.rows.forEach((r,y)=>r.forEach((c,x)=>{g.fillStyle=c.background;g.fillRect(x*cw,y*ch,cw,ch);}));
  frame.rows.forEach((r,y)=>r.forEach((c,x)=>{
    if(c.width===0)return;
    g.save();g.beginPath();g.rect(x*cw,y*ch,cw*c.width,ch);g.clip();
    g.font=`${c.style&2?'italic ':''}${c.style&1?'bold ':''}18px monospace`;
    g.globalAlpha=c.style&16?0.5:1;g.fillStyle=c.foreground;g.fillText(c.text,x*cw,y*ch+2);
    if(c.style&4)g.fillRect(x*cw,y*ch+ch-3,cw*c.width,1);
    if(c.style&8)g.fillRect(x*cw,y*ch+ch/2,cw*c.width,1);
    g.restore();
  }));
  if(frame.cursor_visible){
    const x=frame.cursor_col*cw,y=frame.cursor_row*ch,width=cw*(frame.rows[frame.cursor_row][frame.cursor_col].width===2?2:1);g.fillStyle=g.strokeStyle='#9ac6ff';
    if(frame.cursor_shape==='beam')g.fillRect(x,y,2,ch);
    else if(frame.cursor_shape==='underline')g.fillRect(x,y+ch-2,width,2);
    else g.strokeRect(x+.5,y+.5,width-1,ch-1);
  }
}
