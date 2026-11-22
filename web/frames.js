// One Unicode scalar occupies at most two UTF-16 units.
const singleScalar = text => typeof text === 'string' &&
  (text.length === 1 || (text.length === 2 && text.codePointAt(0) > 0xffff));
const uint = x => Number.isInteger(x) && x >= 1 && x <= 0xffffffff;

function validateStamp(previous, frame, expected) {
  if (!frame || frame.version !== 3 || frame.session !== expected.session ||
      frame.epoch !== expected.epoch || !uint(frame.revision) || !uint(frame.layout)) {
    throw new Error('foreign or invalid frame');
  }
  if (previous && (frame.revision <= previous.revision || frame.layout < previous.layout)) {
    throw new Error('stale frame');
  }
}

function validateRow(row, cols) {
  if (!Array.isArray(row) || row.length !== cols) throw new Error('row bounds');
  const cells = row.map(c => {
    if (!c || !Number.isInteger(c.width) || c.width<0 || c.width>2 || !Number.isInteger(c.style) ||
        c.style<0 || c.style>31 || !singleScalar(c.text) || /[\x00-\x1f\x7f-\x9f\ud800-\udfff]/u.test(c.text) ||
        !/^#[a-f0-9]{6}$/i.test(c.foreground) || !/^#[a-f0-9]{6}$/i.test(c.background)) throw new Error('cell');
    return Object.freeze({text:c.text,foreground:c.foreground,background:c.background,width:c.width,style:c.style});
  });
  for (let i=0;i<cells.length;i++) {
    if(cells[i].width===2 && (i+1===cells.length || cells[i+1].width!==0)) throw new Error('wide lead without continuation');
    if(cells[i].width===0 && (i===0 || cells[i-1].width!==2 || cells[i].text!==' ')) throw new Error('orphan wide continuation');
  }
  return Object.freeze(cells);
}

function finishFrame(frame, cols, rows) {
  if (typeof frame.cursor_visible !== 'boolean' || typeof frame.pending_wrap !== 'boolean' ||
      !['block','underline','beam'].includes(frame.cursor_shape)) throw new Error('cursor visibility');
  if (!Number.isInteger(frame.cursor_row) || !Number.isInteger(frame.cursor_col) ||
      frame.cursor_row<0 || frame.cursor_row>=rows.length || frame.cursor_col<0 || frame.cursor_col>=cols) throw new Error('cursor');
  return Object.freeze({version:3,session:frame.session,epoch:frame.epoch,revision:frame.revision,layout:frame.layout,cols,
    rows:Object.freeze(rows),cursor_row:frame.cursor_row,cursor_col:frame.cursor_col,
    cursor_visible:frame.cursor_visible,cursor_shape:frame.cursor_shape,pending_wrap:frame.pending_wrap});
}

export function acceptFull(previous, frame, expected = {session:'frame-demo',epoch:1}) {
  validateStamp(previous, frame, expected);
  const {cols, rows, layout} = frame;
  if (!Number.isInteger(cols) || cols<1 || cols>256 || !Array.isArray(rows) ||
      rows.length<1 || rows.length>128 || cols*rows.length>32768) throw new Error('dimensions');
  if (previous && layout===previous.layout && (cols!==previous.cols || rows.length!==previous.rows.length)) {
    throw new Error('layout must advance');
  }
  return finishFrame(frame, cols, rows.map(row=>validateRow(row,cols)));
}

export function acceptUpdate(previous, update, expected = {session:'frame-demo',epoch:1}) {
  if (update?.kind === 'snapshot') return acceptFull(previous, update.frame, expected);
  if (update?.kind !== 'delta' || !previous) throw new Error('snapshot required');
  const frame = update.frame;
  validateStamp(previous, frame, expected);
  if (previous.session!==frame.session || previous.epoch!==frame.epoch ||
      frame.base_revision!==previous.revision || frame.layout!==previous.layout) throw new Error('delta base changed');
  if (!Array.isArray(frame.rows) || frame.rows.length > previous.rows.length) throw new Error('delta row count');
  const rows = previous.rows.slice(), seen = new Set();
  for (const row of frame.rows) {
    if (!row || !Number.isInteger(row.index) || row.index<0 || row.index>=rows.length || seen.has(row.index)) {
      throw new Error('invalid or duplicate delta row');
    }
    rows[row.index] = validateRow(row.cells, previous.cols);
    seen.add(row.index);
  }
  return finishFrame(frame, previous.cols, rows);
}
export function paint(canvas, frame, scale = globalThis.devicePixelRatio || 1) {
  if (!Number.isFinite(scale) || scale <= 0 || scale > 4) scale = 1;
  const cw = 12, ch = 24;
  // Cap backing allocation independently of viewport dimensions and device DPR.
  const limit = Math.sqrt(16777216 / (frame.cols * cw * frame.rows.length * ch));
  // Merge only on an unclamped integer DPR: all opaque cell edges then
  // land on physical pixels, without changing fractional-edge rasterization.
  const mergeBackgrounds = Number.isInteger(scale) && scale <= limit;
  scale = Math.min(scale, limit);
  const cssWidth = frame.cols * cw, cssHeight = frame.rows.length * ch;
  const width = Math.max(1, Math.floor(cssWidth * scale));
  const height = Math.max(1, Math.floor(cssHeight * scale));
  // Assigning a dimension clears and resets Canvas even if the value is unchanged.
  if (canvas.width !== width) canvas.width = width;
  if (canvas.height !== height) canvas.height = height;
  if (canvas.style.width !== `${cssWidth}px`) canvas.style.width = `${cssWidth}px`;
  if (canvas.style.height !== `${cssHeight}px`) canvas.style.height = `${cssHeight}px`;
  const g = canvas.getContext('2d');
  g.setTransform(1, 0, 0, 1, 0, 0);
  g.clearRect(0, 0, width, height);
  g.setTransform(scale, 0, 0, scale, 0, 0);
  g.font = '18px monospace'; g.textBaseline = 'top';
  // All backgrounds first: a wide glyph must not be erased by its trailing cell.
  frame.rows.forEach((row, y) => {
    if (!mergeBackgrounds) {
      row.forEach((cell, x) => { g.fillStyle = cell.background; g.fillRect(x*cw, y*ch, cw, ch); });
      return;
    }
    for (let start = 0; start < row.length;) {
      const background = row[start].background;
      let end = start + 1;
      while (end < row.length && row[end].background === background) end++;
      g.fillStyle = background;
      g.fillRect(start*cw, y*ch, (end-start)*cw, ch);
      start = end;
    }
  });
  frame.rows.forEach((r,y)=>r.forEach((c,x)=>{
    if(c.width===0)return;
    g.save();
    try {
      g.beginPath();g.rect(x*cw,y*ch,cw*c.width,ch);g.clip();
      g.font=`${c.style&2?'italic ':''}${c.style&1?'bold ':''}18px monospace`;
      g.globalAlpha=c.style&16?0.5:1;g.fillStyle=c.foreground;g.fillText(c.text,x*cw,y*ch+2);
      if(c.style&4)g.fillRect(x*cw,y*ch+ch-3,cw*c.width,1);
      if(c.style&8)g.fillRect(x*cw,y*ch+ch/2,cw*c.width,1);
    } finally {
      g.restore();
    }
  }));
  if(frame.cursor_visible){
    const x=frame.cursor_col*cw,y=frame.cursor_row*ch,width=cw*(frame.rows[frame.cursor_row][frame.cursor_col].width===2?2:1);g.fillStyle=g.strokeStyle='#9ac6ff';
    if(frame.cursor_shape==='beam')g.fillRect(x,y,2,ch);
    else if(frame.cursor_shape==='underline')g.fillRect(x,y+ch-2,width,2);
    else g.strokeRect(x+.5,y+.5,width-1,ch-1);
  }
}
