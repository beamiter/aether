import {cssColor,sameCellColor} from './cell-colors.js';
import {createRasterCanvasCache} from './raster-canvas-cache.js';
import {CELL_CONCEALED, CELL_OVERLINE, rowMoveForBase} from './frames-v4.js';
import {canRasterizeInk, createTruecolorPainter} from './truecolor-raster.js';

export const CELL_WIDTH = 12, CELL_HEIGHT = 24;

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
    let style = c.style;
    if (c.rendition !== undefined) {
      const rendition = c.rendition;
      if (!Number.isInteger(rendition) || rendition < 0 || rendition > 3) throw new Error('cell rendition');
      if (rendition & 1) style |= CELL_CONCEALED;
      if (rendition & 2) style |= CELL_OVERLINE;
    }
    return Object.freeze({text:c.text,foreground:c.foreground,background:c.background,width:c.width,style});
  });
  for (let i=0;i<cells.length;i++) {
    if(cells[i].width===2 && (i+1===cells.length || cells[i+1].width!==0)) throw new Error('wide lead without continuation');
    if(cells[i].width===0 && (i===0 || cells[i-1].width!==2 || cells[i].text!==' ')) throw new Error('orphan wide continuation');
  }
  return Object.freeze(cells);
}

function finishFrame(frame, cols, rows) {
  if (frame.cursor_color !== undefined && (typeof frame.cursor_color !== 'string' || !/^#[a-f0-9]{6}$/i.test(frame.cursor_color))) throw new Error('cursor color');
  if (typeof frame.cursor_visible !== 'boolean' || typeof frame.pending_wrap !== 'boolean' ||
      !['block','underline','beam'].includes(frame.cursor_shape)) throw new Error('cursor visibility');
  if (!Number.isInteger(frame.cursor_row) || !Number.isInteger(frame.cursor_col) ||
      frame.cursor_row<0 || frame.cursor_row>=rows.length || frame.cursor_col<0 || frame.cursor_col>=cols) throw new Error('cursor');
  return Object.freeze({version:3,session:frame.session,epoch:frame.epoch,revision:frame.revision,layout:frame.layout,cols,
    rows:Object.freeze(rows),cursor_row:frame.cursor_row,cursor_col:frame.cursor_col,
    cursor_visible:frame.cursor_visible,cursor_shape:frame.cursor_shape,cursor_color:frame.cursor_color,pending_wrap:frame.pending_wrap});
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
const painted = new WeakMap();
const rasterCanvas = createRasterCanvasCache(32 * 1024 * 1024);
const glyphCanvases = new WeakMap();
const MIN_CELL_DAMAGE_GAP = 4;
const CELL_DECORATIONS = 4 | 8 | CELL_OVERLINE;
const GLYPH_ATLAS_COLUMNS = 16;
const GLYPH_ATLAS_MIN_DAMAGE = 512;
const GLYPH_ATLAS_SAMPLE = 768;
const CELL_TEXT_TOP = 2;
const paintTruecolor = createTruecolorPainter(CELL_WIDTH,CELL_HEIGHT,CELL_TEXT_TOP);

function paintImages(canvas, g, frame, damage, cw, ch) {
  for (const {y} of damage) for (const {id,column,slice} of frame.rows[y].imageSlices ?? []) {
    const image = frame.images[id];
    const source = rasterCanvas(canvas.ownerDocument,image);
    const x = column * cw, width = image.columns * cw;
    g.save();
    try {
      g.beginPath();g.rect(x,y*ch,width,ch);g.clip();
      g.drawImage(source,x,(y-slice)*ch,width,image.rows*ch);
    } finally {
      g.restore();
    }
  }
}

export function clearPaint(canvas) {
  painted.delete(canvas);
  canvas.width = canvas.width;
}

function paintUnderline(g, cell, x, y, width) {
  const kind = cell.underline ?? (cell.style & 4 || cell.hyperlink_id ? 'single' : null);
  if (!kind) return;
  g.fillStyle = cssColor(cell.underline_color ?? cell.foreground);
  if (kind === 'curly') {
    g.strokeStyle = g.fillStyle;
    g.lineWidth = 1;
    g.beginPath();g.moveTo(x,y);
    for (let offset = 0; offset < width; offset += 4) {
      const rise = offset % 8 === 0 ? -2 : 2;
      g.quadraticCurveTo(x+offset+2,y+rise,x+offset+4,y);
    }
    g.stroke();
  } else if (kind === 'dotted' || kind === 'dashed') {
    const mark = kind === 'dotted' ? 1 : 4;
    for (let offset = 0; offset < width; offset += mark+2) g.fillRect(x+offset,y,Math.min(mark,width-offset),1);
  } else {
    g.fillRect(x,y,width,1);
    if (kind === 'double') g.fillRect(x,y+2,width,1);
  }
}

function paintCellInk(g, cell, x, y) {
  if (cell.width === 0 || (cell.style & CELL_CONCEALED)) return;
  const space = cell.text === ' ';
  if (space && !(cell.style & CELL_DECORATIONS) && !cell.underline && !cell.hyperlink_id) return;
  const width = CELL_WIDTH * cell.width;
  g.save();
  try {
    g.beginPath();g.rect(x,y,width,CELL_HEIGHT);g.clip();
    g.globalAlpha = cell.style & 16 ? 0.5 : 1;
    g.fillStyle = cssColor(cell.foreground);
    if (!space) {
      g.font = `${cell.style&2?'italic ':''}${cell.style&1?'bold ':''}18px monospace`;
      g.fillText(cell.text,x,y+CELL_TEXT_TOP);
    }
    if (cell.style & CELL_OVERLINE) g.fillRect(x,y+1,width,1);
    if (cell.style & 8) g.fillRect(x,y+CELL_HEIGHT/2,width,1);
    paintUnderline(g,cell,x,y+CELL_HEIGHT-3,width);
  } finally {
    g.restore();
  }
}

function buildGlyphAtlas(canvas, g, frame, damage, scale) {
  const document = canvas.ownerDocument;
  if (!document || typeof g.drawImage !== 'function' ||
      g.globalAlpha !== 1 || g.globalCompositeOperation !== 'source-over') return null;
  let damagedCells = 0;
  for (const {first,last} of damage) damagedCells += last - first;
  if (damagedCells < GLYPH_ATLAS_MIN_DAMAGE) return null;

  // Limit the raster to the size of a 16 by 16 atlas at 2x DPR.
  const atlasRows = Math.floor(GLYPH_ATLAS_COLUMNS * 4 / (scale * scale));
  const capacity = GLYPH_ATLAS_COLUMNS * atlasRows;
  const slots = new Uint16Array(frame.cols * frame.rows.length);
  const tiles = [], keys = new Map();
  let sampled = 0, repeated = 0;
  for (const {y,first,last} of damage) {
    const row = frame.rows[y];
    for (let x = first, limit = Math.min(last,row.length); x < limit; x++) {
      if (sampled++ === GLYPH_ATLAS_SAMPLE && repeated < GLYPH_ATLAS_SAMPLE / 4) return null;
      const cell = row[x];
      // Each tile is opaque and owns one cell of width. Paint combining
      // sequences, wide glyphs and decorations directly on the terminal.
      if (cell.width !== 1 || (cell.style & ~(1 | 2 | 16)) || cell.underline || cell.hyperlink_id ||
          cell.text.length !== 1 || cell.text < '!' || cell.text > '~') continue;
      const key = `${cell.foreground},${cell.background}:${cell.style}:${cell.text}`;
      let slot = keys.get(key);
      if (slot !== undefined) repeated++;
      else {
        if (tiles.length === capacity) continue;
        slot = tiles.length;
        keys.set(key,slot);
        tiles.push(cell);
      }
      slots[y*frame.cols+x] = slot+1;
    }
  }
  if (repeated < sampled / 4) return null;

  let source = glyphCanvases.get(document);
  if (!source) {
    source = document.createElement('canvas');
    glyphCanvases.set(document,source);
  }
  const width = CELL_WIDTH * scale, height = CELL_HEIGHT * scale;
  const textureWidth = GLYPH_ATLAS_COLUMNS * width;
  const textureHeight = Math.ceil(tiles.length / GLYPH_ATLAS_COLUMNS) * height;
  if (source.width !== textureWidth) source.width = textureWidth;
  if (source.height !== textureHeight) source.height = textureHeight;
  const language = canvas.closest?.('[lang]')?.lang ?? document.documentElement?.lang ?? '';
  if (source.lang !== language) source.lang = language;
  const texture = source.getContext('2d');
  if (!texture) return null;
  texture.setTransform(scale,0,0,scale,0,0);
  texture.textBaseline = g.textBaseline;
  texture.textAlign = g.textAlign;
  texture.direction = g.direction;
  for (const [slot,cell] of tiles.entries()) {
    const x = slot % GLYPH_ATLAS_COLUMNS * CELL_WIDTH;
    const y = Math.floor(slot / GLYPH_ATLAS_COLUMNS) * CELL_HEIGHT;
    texture.fillStyle = cssColor(cell.background);
    texture.fillRect(x,y,CELL_WIDTH,CELL_HEIGHT);
    paintCellInk(texture,cell,x,y);
  }
  return {source,slots,width,height};
}

function sameCellPaint(before, after) {
  return before === after || (before.text === after.text && before.width === after.width &&
    sameCellColor(before.background,after.background) && sameCellColor(before.foreground,after.foreground) &&
    before.style === after.style && before.underline === after.underline &&
    before.underline_color === after.underline_color && before.hyperlink_id === after.hyperlink_id);
}

function cursorWidth(frame) {
  return frame.rows[frame.cursor_row][frame.cursor_col].width === 2 ? 2 : 1;
}

function appendCellDamage(damage, previous, frame, y, oldCursorRow, newCursorRow) {
  const before = previous.rows[y], after = frame.rows[y], cols = frame.cols;
  const oldCol = y === oldCursorRow ? previous.cursor_col : -1;
  const newCol = y === newCursorRow ? frame.cursor_col : -1;
  const start = before !== after ? 0 : Math.min(oldCol < 0 ? cols : oldCol, newCol < 0 ? cols : newCol);
  const end = before !== after ? cols : Math.max(oldCol, newCol) + 1;
  let first = -1, last = 0;
  for (let x = start; x < end;) {
    if (x !== oldCol && x !== newCol && sameCellPaint(before[x], after[x])) {
      x++;
      continue;
    }
    // A boundary that bisects an old or new wide glyph cannot separate paints.
    let left = x, right = x + 1;
    while (left > last && (before[left].width === 0 || after[left].width === 0)) left--;
    while (right < cols && (before[right].width === 0 || after[right].width === 0)) right++;
    // Amortize the extra Canvas operations over a sufficiently large unchanged gap.
    if (first === -1) first = left;
    else if (left - last >= MIN_CELL_DAMAGE_GAP) {
      damage.push({y, first, last});
      first = left;
    }
    last = right;
    x = right;
  }
  if (first !== -1) damage.push({y, first, last});
}

export function backingGeometry(rows, cols, scale = globalThis.devicePixelRatio || 1) {
  if (!Number.isFinite(scale) || scale <= 0 || scale > 4) scale = 1;
  const cssWidth = cols * CELL_WIDTH, cssHeight = rows * CELL_HEIGHT;
  // Cap backing allocation independently of viewport dimensions and device DPR.
  const limit = Math.sqrt(16777216 / (cssWidth * cssHeight));
  // Merge only on an unclamped integer DPR: all opaque cell edges then
  // land on physical pixels, without changing fractional-edge rasterization.
  const mergeBackgrounds = Number.isInteger(scale) && scale <= limit;
  scale = Math.min(scale, limit);
  const width = Math.max(1, Math.floor(cssWidth * scale));
  const height = Math.max(1, Math.floor(cssHeight * scale));
  return {width, height, cssWidth, cssHeight, scale, mergeBackgrounds};
}

function paintBackgrounds(g, frame, damage, merge) {
  const cw = CELL_WIDTH, ch = CELL_HEIGHT;
  for (const {y,first,last} of damage) {
    const row = frame.rows[y];
    const limit = Math.min(last,row.length);
    for (let start=first; start<limit;) {
      const background = row[start].background;
      let end = start+1;
      if (merge) while (end<limit && row[end].background === background) end++;
      g.fillStyle = cssColor(background);
      g.fillRect(start*cw,y*ch,(end-start)*cw,ch);
      start = end;
    }
  }
}

export function paint(canvas, frame, deviceScale = globalThis.devicePixelRatio || 1) {
  const cw = CELL_WIDTH, ch = CELL_HEIGHT;
  const {width, height, cssWidth, cssHeight, scale, mergeBackgrounds} =
    backingGeometry(frame.rows.length, frame.cols, deviceScale);
  const last = painted.get(canvas);
  const previous = last?.frame;
  const rowPixels = ch * scale;
  const cellPixels = cw * scale;
  // Fractional row edges can blend with the next row in Canvas2D. Only
  // clear individual rows when their boundaries align to backing pixels.
  const full = !last || last.scale !== scale || !Number.isInteger(rowPixels) ||
    canvas.width !== width || canvas.height !== height ||
    previous.session !== frame.session || previous.epoch !== frame.epoch ||
    previous.layout !== frame.layout || previous.screen !== frame.screen || previous.cols !== frame.cols ||
    previous.rows.length !== frame.rows.length || previous.selection !== frame.selection;
  const cursorMoved = !full && (previous.cursor_row !== frame.cursor_row ||
    previous.cursor_col !== frame.cursor_col || previous.cursor_shape !== frame.cursor_shape ||
    previous.cursor_visible !== frame.cursor_visible || previous.cursor_color !== frame.cursor_color);
  const oldCursorRow = cursorMoved && previous.cursor_visible ? previous.cursor_row : -1;
  const newCursorRow = cursorMoved && frame.cursor_visible ? frame.cursor_row : -1;
  const canPaintCells = !full && !frame.selection && Number.isInteger(cellPixels);
  const move = !full && mergeBackgrounds && !frame.selection
    ? rowMoveForBase(frame,previous.revision) : undefined;
  // Only an already-sized backing can have a matching row-copy base.
  const moveContext = move ? canvas.getContext('2d') : null;
  const canCopyRows = typeof moveContext?.drawImage === 'function';
  let copy = false;
  const damage = [];
  for (let y = 0; y < frame.rows.length; y++) {
    const before = previous?.rows[y], after = frame.rows[y];
    const changed = full || before !== after || y === oldCursorRow || y === newCursorRow;
    const inMove = canCopyRows && y >= move.to && y < move.to + move.count;
    if (inMove) {
      const from = move.from + y - move.to;
      if (after === previous.rows[from] && !after.imageSlices?.length &&
          !(previous.cursor_visible && previous.cursor_row === from) &&
          !(frame.cursor_visible && frame.cursor_row === y)) {
        copy ||= changed;
        continue;
      }
      // The copy can cover this row with source images or an old cursor.
    } else if (!changed) continue;
    if (canPaintCells && !inMove && before.length === frame.cols && after.length === frame.cols &&
        !before.imageSlices?.length && !after.imageSlices?.length) {
      appendCellDamage(damage, previous, frame, y, oldCursorRow, newCursorRow);
    } else {
      damage.push({y, first: 0, last: frame.cols});
    }
  }
  if (!damage.length && !copy) {
    painted.set(canvas, {frame, scale});
    return;
  }
  // Canvas mutations after this point can fail partway. A recovery must
  // perform a full repaint even if it has the same grid and row references.
  painted.delete(canvas);
  // Assigning a dimension clears and resets Canvas even if the value is unchanged.
  if (canvas.width !== width) canvas.width = width;
  if (canvas.height !== height) canvas.height = height;
  if (canvas.style.width !== `${cssWidth}px`) canvas.style.width = `${cssWidth}px`;
  if (canvas.style.height !== `${cssHeight}px`) canvas.style.height = `${cssHeight}px`;
  const g = moveContext ?? canvas.getContext('2d');
  g.setTransform(1, 0, 0, 1, 0, 0);
  if (copy) {
    // A single draw reads its source before writing the overlapping destination.
    g.drawImage(canvas,0,move.from*rowPixels,width,move.count*rowPixels,
      0,move.to*rowPixels,width,move.count*rowPixels);
  }
  if (damage.length === frame.rows.length && damage.every(span => span.first === 0 && span.last === frame.cols)) {
    g.clearRect(0, 0, width, height);
  } else for (const {y,first,last} of damage) {
    const left = first * cellPixels, right = last === frame.cols ? width : last * cellPixels;
    g.clearRect(left, y * rowPixels, right - left, rowPixels);
  }
  g.setTransform(scale, 0, 0, scale, 0, 0);
  g.font = '18px monospace'; g.textBaseline = 'top';
  // Integer backing pixels allow a pixel-for-pixel copy of an opaque tile.
  const atlas = mergeBackgrounds ? buildGlyphAtlas(canvas,g,frame,damage,scale) : null;
  const rasterized = mergeBackgrounds && !atlas && paintTruecolor(canvas,g,frame,damage,scale);
  // A wide glyph can cover its trailing cell, so its background goes down first.
  if (!rasterized) paintBackgrounds(g,frame,damage,mergeBackgrounds);
  for (const {y,first,last} of damage) {
    const row = frame.rows[y];
    for (let x = first, limit = Math.min(last, row.length); x < limit; x++) {
      const index = atlas?.slots[y*frame.cols+x];
      if (index) {
        const {source,width,height} = atlas;
        const slot = index-1;
        const left = slot % GLYPH_ATLAS_COLUMNS * width;
        const top = Math.floor(slot / GLYPH_ATLAS_COLUMNS) * height;
        g.drawImage(source,left,top,width,height,x*cw,y*ch,cw,ch);
      } else if (!rasterized || !canRasterizeInk(row[x])) {
        paintCellInk(g,row[x],x*cw,y*ch);
      }
    }
  }
  paintImages(canvas,g,frame,damage,cw,ch);
  if (frame.selection) {
    g.fillStyle = '#78a8f46b';
    for (const {row,first,last} of frame.selection) g.fillRect(first*cw,row*ch,(last-first)*cw,ch);
  }
  if(frame.cursor_visible && damage.some(({y,first,last}) =>
      y===frame.cursor_row && frame.cursor_col>=first && frame.cursor_col<last)){
    const x=frame.cursor_col*cw,y=frame.cursor_row*ch,width=cw*cursorWidth(frame);g.fillStyle=g.strokeStyle=frame.cursor_color ?? '#9ac6ff';
    if(frame.cursor_shape==='beam')g.fillRect(x,y,2,ch);
    else if(frame.cursor_shape==='underline')g.fillRect(x,y+ch-2,width,2);
    else g.strokeRect(x+.5,y+.5,width-1,ch-1);
  }
  painted.set(canvas, {frame, scale});
}
