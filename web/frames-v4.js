export const CHANNEL_IMAGE_VERSION = 5;
export const CHANNEL_FRAME_VERSION = 6;
export const CHANNEL_PACKED_VERSION = 7;
export const CHANNEL_RENDITION_VERSION = 8;
export const CHANNEL_DENSE_VERSION = 9;
export const SSH_DENSE_FRAME_VERSION = CHANNEL_DENSE_VERSION;
// Decoded cells keep bit 2 available for the version-3 underline flag.
export const CELL_CONCEALED = 256;
export const CELL_OVERLINE = 128;
const textChannelVersion = 4;
const maxImages = 32;
const maxImageBytes = 2 * 1024 * 1024;
const maxImageDimension = 4096;
const maxEncodedImageBytes = 4 * Math.ceil((maxImageBytes + 2 * maxImages) / 3);
const base64Alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
const maxCounter = 18446744073709551615n;
const maxEncodedBytes = 8 * 1024 * 1024;
const maxTextBytes = 512 * 1024;
const maxClusterBytes = 64;
const maxHyperlinks = 4096;
const maxHyperlinkBytes = 1024 * 1024;
const maxHyperlinkTargetBytes = 2048;
const underlines = new Set(['single','double','curly','dotted','dashed']);
const colors = /^#[a-fA-F0-9]{6}$/;
const invalidCellText = /[\x00-\x1f\x7f-\x9f\ud800-\udfff]/u;
const nonAscii = /[^\x00-\x7f]/;
const wireBlockUnits = 8192;
const supportsPackedRows = version => version === CHANNEL_PACKED_VERSION || version === CHANNEL_RENDITION_VERSION;
const supportsRowMoves = version => version === CHANNEL_FRAME_VERSION ||
  version === CHANNEL_DENSE_VERSION || supportsPackedRows(version);
const supportsChannelImages = version => version === CHANNEL_IMAGE_VERSION || supportsRowMoves(version);
export const supportsChannelRendition = version => version === CHANNEL_RENDITION_VERSION || version === CHANNEL_DENSE_VERSION;
const frameMoves = new WeakMap();
const incomingRasters = new WeakMap();
const retainedRasters = new WeakMap();
const binaryDecoder = new TextDecoder('utf-8',{fatal:true});

function packetImageBytes(packet) {
  const size = packet?.image_bytes;
  if (size === undefined) return 0;
  if (!Number.isInteger(size) || size < 4 || size > maxImageBytes || size % 4 !== 0 ||
      !packet.frame || typeof packet.frame !== 'object' || Array.isArray(packet.frame))
    throw new Error('invalid binary Channel image length');
  return size;
}

export function decodeLocalChannelEvent(message) {
  if (!(message instanceof ArrayBuffer)) return message;
  // Active and preview RGBA8, event JSON, then its LE length and "JTR1".
  const trailer = message.byteLength - 8;
  const view = new DataView(message);
  if (trailer < 0 || view.getUint32(trailer+4,true) !== 0x3152544a)
    throw new Error('invalid binary Channel trailer');
  const metadataBytes = view.getUint32(trailer,true);
  const pixels = trailer - metadataBytes;
  if (metadataBytes === 0 || metadataBytes > 2*(maxEncodedBytes+maxEncodedImageBytes) ||
      pixels < 1 || pixels > 2*maxImageBytes)
    throw new Error('binary Channel byte limit');
  const event = JSON.parse(binaryDecoder.decode(new Uint8Array(message,pixels,metadataBytes)));
  const active = packetImageBytes(event), preview = packetImageBytes(event?.preview);
  if (event?.kind !== 'update' || active + preview !== pixels)
    throw new Error('invalid binary Channel image ranges');
  if (active) incomingRasters.set(event.frame,new Uint8Array(message,0,active));
  if (preview) incomingRasters.set(event.preview.frame,new Uint8Array(message,active,preview));
  return event;
}

export function channelRasterBytes(resource) {
  return retainedRasters.get(resource);
}

export function rowMoveForBase(frame, revision) {
  const move = frameMoves.get(frame);
  return move?.base === revision ? move : undefined;
}

export function channelCounter(value) {
  if (typeof value !== 'string' || !/^[1-9][0-9]{0,19}$/.test(value)) throw new Error('invalid Channel counter');
  const exact = BigInt(value);
  if (exact > maxCounter) throw new Error('Channel counter overflow');
  return exact;
}

function utf8Bytes(text) {
  let bytes = 0;
  for (let i = 0; i < text.length; i++) {
    const point = text.codePointAt(i);
    if (point > 0xffff) { bytes += 4; i++; }
    else bytes += point < 0x80 ? 1 : point < 0x800 ? 2 : 3;
  }
  return bytes;
}

function encodedUtf8Bytes(text) {
  let bytes = 0;
  for (let start=0; start<text.length;) {
    let end = Math.min(start+wireBlockUnits,text.length);
    const last = text.charCodeAt(end-1);
    // Keep a UTF-16 pair together when a block boundary falls inside one.
    if (end < text.length && last >= 0xd800 && last <= 0xdbff) end--;
    const block = text.slice(start,end);
    const first = block.search(nonAscii);
    bytes += first < 0 ? block.length : first + utf8Bytes(block.slice(first));
    start = end;
  }
  return bytes;
}

function validatedHyperlinks(entries, previous) {
  const links = new Map();
  if (entries === undefined) return links;
  if (!Array.isArray(entries) || entries.length > maxHyperlinks) throw new Error('Channel hyperlink count');
  let bytes = 0;
  for (const entry of entries) {
    const id = entry?.id, target = entry?.target;
    if (!Number.isInteger(id) || id < 1 || id > maxHyperlinks || links.has(id) ||
        typeof target !== 'string' || target.length > maxHyperlinkTargetBytes ||
        !/^https?:\/\/[^/?#]+/i.test(target) || /[\s\\\x00-\x1f\x7f-\x9f\ud800-\udfff]/u.test(target))
      throw new Error('invalid Channel hyperlink');
    const url = new URL(target);
    if (!url.hostname || url.username || url.password) throw new Error('invalid Channel hyperlink authority');
    const size = utf8Bytes(target);
    bytes += size;
    if (size > maxHyperlinkTargetBytes || bytes > maxHyperlinkBytes) throw new Error('Channel hyperlink byte limit');
    const prior = previous?.hyperlinks[id];
    if (prior !== undefined && prior !== target) throw new Error('changed Channel hyperlink identity');
    links.set(id,target);
  }
  return links;
}

function sameRaster(prior, rgba, raster) {
  const retained = retainedRasters.get(prior);
  if (!retained && !raster) return prior.rgba === rgba;
  const a = retained ?? decodeBase64Bytes(prior.rgba);
  const b = raster ?? decodeBase64Bytes(rgba);
  if (a.length !== b.length) return false;
  // Raster ranges start on RGBA pixel boundaries.
  const left = new Uint32Array(a.buffer,a.byteOffset,a.byteLength/4);
  const right = new Uint32Array(b.buffer,b.byteOffset,b.byteLength/4);
  for (let i=0; i<left.length; i++) if (left[i] !== right[i]) return false;
  return true;
}

function validatedImages(entries, supportsImages, base, previous, pixels) {
  const images = new Map();
  if (entries === undefined && !pixels) return {images, encodedBytes: 0};
  if (!supportsImages || !Array.isArray(entries) || entries.length > maxImages)
    throw new Error('Channel image count or version');
  let bytes = 0, encodedBytes = 0;
  for (const entry of entries) {
    if (!entry || typeof entry !== 'object') throw new Error('invalid Channel image');
    const {id, width, height, columns, rows, rgba} = entry;
    channelCounter(id);
    if (images.has(id) || base?.images[id] ||
        !Number.isInteger(width) || width < 1 || width > maxImageDimension ||
        !Number.isInteger(height) || height < 1 || height > maxImageDimension ||
        !Number.isInteger(columns) || columns < 1 || columns > 256 ||
        !Number.isInteger(rows) || rows < 1 || rows > 128) throw new Error('invalid Channel image geometry or identity');
    const size = width * height * 4;
    const start = bytes;
    bytes += size;
    if (bytes > maxImageBytes) throw new Error('Channel image byte limit');
    let raster;
    if (pixels) {
      if (rgba !== undefined || bytes > pixels.length) throw new Error('invalid binary Channel image');
      raster = pixels.subarray(start,bytes);
    } else {
      if (typeof rgba !== 'string' || rgba.length !== 4 * Math.ceil(size / 3))
        throw new Error('Channel image byte limit');
      const padding = (3 - size % 3) % 3;
      const end = rgba.length - padding;
      const last = base64Alphabet.indexOf(rgba[end - 1]);
      if (/[^a-zA-Z0-9+/]/.test(rgba.slice(0,end)) || rgba.slice(end) !== '='.repeat(padding) ||
          (padding && (last & (padding === 1 ? 3 : 15)))) throw new Error('invalid Channel RGBA base64');
      encodedBytes += rgba.length;
    }
    const prior = previous?.images[id];
    if (prior && (prior.width !== width || prior.height !== height || prior.columns !== columns ||
        prior.rows !== rows || !sameRaster(prior,rgba,raster))) throw new Error('changed Channel image identity');
    const image = prior ?? Object.freeze({id,width,height,columns,rows,rgba,bytes:size});
    if (raster && !prior) {
      // Detach the retained image from the IPC envelope and the other pane's bytes.
      retainedRasters.set(image,new Uint8ClampedArray(raster));
    }
    images.set(id,image);
  }
  if (pixels && bytes !== pixels.length) throw new Error('invalid binary Channel image length');
  return {images, encodedBytes};
}

function validateSlices(slices, cols, supportsImages) {
  if (slices === undefined) return undefined;
  if (!supportsImages || !Array.isArray(slices) || slices.length > cols)
    throw new Error('invalid Channel image slices');
  let last = -1;
  return Object.freeze(slices.map(entry => {
    const {column, id, slice} = entry ?? {};
    if (!Number.isInteger(column) || column <= last || column >= cols ||
        !Number.isInteger(slice) || slice < 0 || slice >= 128) throw new Error('invalid Channel image slice');
    channelCounter(id);
    last = column;
    return Object.freeze({column,id,slice});
  }));
}

function resolveImages(rows, cols, newImages, base, clipImages = false) {
  const images = Object.create(null);
  const referenced = new Set();
  let bytes = 0, count = 0;
  for (const row of rows) {
    for (const {id,column,slice} of row.imageSlices ?? []) {
      const resource = newImages.get(id) ?? base?.images[id];
      if (!resource || slice >= resource.rows || (!clipImages && column + resource.columns > cols))
        throw new Error('unresolved or out-of-bounds Channel image slice');
      if (newImages.has(id)) referenced.add(id);
      if (images[id]) continue;
      if (++count > maxImages || (bytes += resource.bytes) > maxImageBytes)
        throw new Error('Channel retained image limit');
      images[id] = resource;
    }
  }
  if (referenced.size !== newImages.size) throw new Error('unreferenced Channel images');
  return Object.freeze(images);
}

function validStyle(cell, links, extended) {
  return cell && Number.isInteger(cell.style) && cell.style >= 0 &&
    (extended ? cell.style <= 255 : cell.style <= 123 && (cell.style & 4) === 0) &&
    typeof cell.foreground === 'string' && colors.test(cell.foreground) &&
    typeof cell.background === 'string' && colors.test(cell.background) &&
    (cell.underline === undefined || underlines.has(cell.underline)) &&
    (cell.underline_color === undefined || (typeof cell.underline_color === 'string' && colors.test(cell.underline_color))) &&
    (cell.hyperlink_id === undefined || links.has(cell.hyperlink_id));
}

function decodedStyle(style, extended) {
  return extended && (style & 4) ? (style & ~4) | CELL_CONCEALED : style;
}

const hexByte = Array.from({length:256}, (_, value) => value.toString(16).padStart(2, '0'));
const hexStart = hexByte.map(value=>'#'+value);
const emptyAttributes = Object.freeze({underline:undefined,underline_color:undefined,hyperlink_id:undefined});
const hasBase64Bytes = typeof Uint8Array.fromBase64 === 'function';

function decodeBase64Bytes(encoded) {
  if (hasBase64Bytes) return Uint8Array.fromBase64(encoded);
  const text = atob(encoded);
  const bytes = new Uint8Array(text.length);
  for (let i=0; i<text.length; i++) bytes[i] = text.charCodeAt(i);
  return bytes;
}

function validateDenseRow(row, cols, links, referenced, supportsImages) {
  const {wrapped, columns, cell_bytes, text, attributes = []} = row;
  if (typeof wrapped !== 'boolean' || columns !== cols || row.cells !== undefined ||
      row.styles !== undefined || row.runs !== undefined || typeof cell_bytes !== 'string' ||
      cell_bytes.length !== cols * 12 || /[^a-zA-Z0-9+/]/.test(cell_bytes) ||
      typeof text !== 'string' || text.length > cols * maxClusterBytes ||
      !Array.isArray(attributes) || attributes.length > cols) throw new Error('invalid dense Channel row');
  const bytes = decodeBase64Bytes(cell_bytes);
  const styles = attributes.length ? new Map() : undefined;
  for (const attribute of attributes) {
    const {column, underline, underline_color, hyperlink_id} = attribute ?? {};
    if (!Number.isInteger(column) || column < 0 || column >= cols || styles.has(column) ||
        (underline === undefined && underline_color === undefined && hyperlink_id === undefined) ||
        (underline !== undefined && !underlines.has(underline)) ||
        (underline_color !== undefined && (typeof underline_color !== 'string' || !colors.test(underline_color))) ||
        (hyperlink_id !== undefined && !links.has(hyperlink_id))) throw new Error('invalid dense Channel attributes');
    styles.set(column, {underline,underline_color,hyperlink_id});
  }
  const cells = [], rowLinks = new Set();
  let offset = 0, textBytes = 0, lead = false;
  let previousBackground = -1, background;
  for (let column = 0; column < cols; column++) {
    const start = column * 9;
    const r = bytes[start], g = bytes[start+1], b = bytes[start+2];
    const br = bytes[start+3], bg = bytes[start+4], bb = bytes[start+5];
    const foreground = hexStart[r] + hexByte[g] + hexByte[b];
    const backgroundKey = (br << 16) | (bg << 8) | bb;
    if (backgroundKey !== previousBackground) {
      background = hexStart[br] + hexByte[bg] + hexByte[bb];
      previousBackground = backgroundKey;
    }
    const style = decodedStyle(bytes[start+6],true);
    const width = bytes[start+7], units = bytes[start+8];
    if (width > 2 || units === 0 || units > maxClusterBytes || offset + units > text.length)
      throw new Error('invalid dense Channel cell');
    const glyph = units === 1 ? text[offset] : text.slice(offset,offset+units);
    const code = units === 1 ? text.charCodeAt(offset) : 0;
    offset += units;
    if (code >= 0x20 && code <= 0x7e) textBytes++;
    else {
      const size = utf8Bytes(glyph);
      if (size > maxClusterBytes || invalidCellText.test(glyph)) throw new Error('invalid dense Channel text');
      textBytes += size;
    }
    if (lead !== (width === 0) || (width === 0 && glyph !== ' ')) throw new Error('invalid Channel wide span');
    lead = width === 2;
    const {underline,underline_color,hyperlink_id} = styles?.get(column) ?? emptyAttributes;
    if (hyperlink_id !== undefined) {
      rowLinks.add(hyperlink_id);
      referenced.add(hyperlink_id);
    }
    cells.push(Object.freeze({text:glyph,width,style,foreground,background,underline,underline_color,hyperlink_id}));
  }
  if (lead || offset !== text.length) throw new Error('invalid dense Channel row width');
  const imageSlices = validateSlices(row.image_slices, cols, supportsImages);
  return Object.freeze(Object.assign(cells, {wrapped,textBytes,links:Object.freeze([...rowLinks]),imageSlices}));
}

function packedCell(text, width, {foreground,background,style,underline,underline_color,hyperlink_id}) {
  return Object.freeze({text,foreground,background,width,style,underline,underline_color,hyperlink_id});
}

function validatePackedRow(row, cols, links, referenced, supportsImages, extended) {
  if (!row || typeof row.wrapped !== 'boolean' || row.columns !== cols || row.cells !== undefined ||
      !Array.isArray(row.styles) || row.styles.length > cols || !Array.isArray(row.runs) || row.runs.length > cols)
    throw new Error('invalid packed Channel row');
  const styles = row.styles.map(paint => {
    if (!validStyle(paint, links, extended)) throw new Error('invalid packed Channel style');
    const {foreground,background,style,underline,underline_color,hyperlink_id} = paint;
    return {foreground,background,style:decodedStyle(style,extended),underline,underline_color,hyperlink_id};
  });
  const cells = [], blanks = [], usedStyles = new Set(), rowLinks = new Set();
  let textBytes = 0, lead = false;
  const append = (text, width, paint) => {
    if (cells.length >= cols) throw new Error('packed Channel row width');
    if (lead !== (width === 0) || (width === 0 && text !== ' '))
      throw new Error('invalid Channel wide span');
    lead = width === 2;
    cells.push(packedCell(text,width,paint));
  };
  for (const run of row.runs) {
    if (!Array.isArray(run) || (run.length !== 2 && run.length !== 3)) throw new Error('invalid packed Channel run');
    const [index, text, width] = run, explicit = run.length === 3;
    if (!Number.isInteger(index) || index < 0 || index >= styles.length ||
        typeof text !== 'string' || !text.length ||
        text.length > (explicit ? maxClusterBytes : (cols - cells.length) * 2) || invalidCellText.test(text) ||
        (explicit && (!Number.isInteger(width) || width < 0 || width > 2)))
      throw new Error('invalid packed Channel run');
    const paint = styles[index], size = utf8Bytes(text);
    if (explicit && size > maxClusterBytes) throw new Error('Channel cluster byte limit');
    textBytes += size;
    if (explicit || text.length === 1) append(text,explicit ? width : 1,paint);
    else if (size === text.length) {
      // The bytes and UTF-16 counts can only match for an ASCII run.
      if (cells.length+text.length > cols) throw new Error('packed Channel row width');
      if (lead) throw new Error('invalid Channel wide span');
      for (let i=0; i<text.length; i++) {
        const glyph = text[i];
        cells.push(glyph === ' ' ? (blanks[index] ??= packedCell(glyph,1,paint)) : packedCell(glyph,1,paint));
      }
    } else for (const scalar of text) append(scalar,1,paint);
    usedStyles.add(index);
    if (paint.hyperlink_id !== undefined) {
      rowLinks.add(paint.hyperlink_id);
      referenced.add(paint.hyperlink_id);
    }
  }
  if (lead) throw new Error('invalid Channel wide span');
  if (cells.length !== cols || usedStyles.size !== styles.length) throw new Error('invalid packed Channel row width or styles');
  const imageSlices = validateSlices(row.image_slices, cols, supportsImages);
  return Object.freeze(Object.assign(cells, {wrapped: row.wrapped, textBytes, links:Object.freeze([...rowLinks]), imageSlices}));
}

function validateRow(row, cols, links, referenced, supportsImages, version) {
  const dense = version === CHANNEL_DENSE_VERSION;
  if (row?.cell_bytes !== undefined) {
    if (!dense) throw new Error('dense Channel row version');
    return validateDenseRow(row, cols, links, referenced, supportsImages);
  }
  const packed = dense || supportsPackedRows(version), extended = supportsChannelRendition(version);
  if (packed && row?.cells === undefined) return validatePackedRow(row, cols, links, referenced, supportsImages, extended);
  if (packed && (row.styles !== undefined || row.runs !== undefined || row.columns !== undefined))
    throw new Error('mixed Channel row encodings');
  if (!row || typeof row.wrapped !== 'boolean' || !Array.isArray(row.cells) || row.cells.length !== cols)
    throw new Error('invalid Channel row');
  let textBytes = 0;
  const rowLinks = new Set();
  const cells = row.cells.map((cell, i) => {
    if (!validStyle(cell, links, extended) || !Number.isInteger(cell.width) || cell.width < 0 || cell.width > 2 ||
        typeof cell.text !== 'string' || !cell.text.length || cell.text.length > maxClusterBytes || invalidCellText.test(cell.text))
      throw new Error('invalid Channel cell');
    const size = utf8Bytes(cell.text);
    if (size > maxClusterBytes) throw new Error('Channel cluster byte limit');
    textBytes += size;
    if ((cell.width === 2 && row.cells[i + 1]?.width !== 0) ||
        (cell.width === 0 && (row.cells[i - 1]?.width !== 2 || cell.text !== ' ')))
      throw new Error('invalid Channel wide span');
    const {text, foreground, background, width, style, underline, underline_color, hyperlink_id} = cell;
    if (hyperlink_id !== undefined) {
      rowLinks.add(hyperlink_id);
      referenced.add(hyperlink_id);
    }
    return Object.freeze({text, foreground, background, width, style:decodedStyle(style,extended), underline, underline_color, hyperlink_id});
  });
  const imageSlices = validateSlices(row.image_slices, cols, supportsImages);
  return Object.freeze(Object.assign(cells, {wrapped: row.wrapped, textBytes, links:Object.freeze([...rowLinks]), imageSlices}));
}

export function acceptChannelFrame(previous, encoded, expected, snapshotOnly = false) {
  const expectedVersion = expected.frameVersion ?? previous?.version;
  const limit = maxEncodedBytes + (supportsChannelImages(expectedVersion) ? maxEncodedImageBytes : 0);
  if (typeof encoded !== 'string' || encoded.length > limit) throw new Error('Channel frame byte limit');
  const wireBytes = encodedUtf8Bytes(encoded);
  if (wireBytes > limit) throw new Error('Channel frame byte limit');
  return acceptChannelUpdate(previous,JSON.parse(encoded),expected,snapshotOnly,wireBytes);
}

export function acceptChannelPayload(previous, {frame,frame_bytes}, expected, snapshotOnly = false) {
  return typeof frame === 'string'
    ? acceptChannelFrame(previous,frame,expected,snapshotOnly)
    : acceptChannelUpdate(previous,frame,expected,snapshotOnly,frame_bytes);
}

export function acceptChannelUpdate(previous, update, expected, snapshotOnly, wireBytes) {
  if (!Number.isSafeInteger(wireBytes) || wireBytes < 1) throw new Error('Channel frame byte limit');
  const expectedVersion = expected.frameVersion ?? previous?.version;
  const stamp = update?.stamp;
  if (!stamp || (stamp.version !== textChannelVersion && !supportsChannelImages(stamp.version)) ||
      (expectedVersion !== undefined && stamp.version !== expectedVersion) || stamp.session !== expected.session ||
      stamp.epoch !== String(expected.epoch) || stamp.resource_generation !== stamp.epoch ||
      !['primary','alternate'].includes(stamp.screen))
    throw new Error('foreign Channel frame');
  channelCounter(stamp.epoch);
  const revision = channelCounter(stamp.revision), layout = channelCounter(stamp.layout);
  if (previous && (previous.epoch !== expected.epoch || previous.session !== expected.session ||
      revision <= channelCounter(previous.revision) || layout < channelCounter(previous.layout)))
    throw new Error('stale Channel frame');
  if (!Array.isArray(update.rows) || update.rows.length > 128) throw new Error('Channel row bounds');
  const moves = update.moves === undefined ? [] : update.moves;
  if (update.moves !== undefined && (!supportsRowMoves(stamp.version) || update.kind !== 'delta' ||
      !Array.isArray(moves) || moves.length > 128)) throw new Error('invalid Channel row moves');
  const supportsImages = supportsChannelImages(stamp.version);
  const imageBase = update.kind === 'delta' ? previous : null;
  const {images: newImages, encodedBytes} = validatedImages(update.images,
    supportsImages && stamp.screen === 'primary', imageBase, previous, incomingRasters.get(update));
  if (wireBytes > maxEncodedBytes + encodedBytes) throw new Error('Channel frame byte limit');
  const links = validatedHyperlinks(update.hyperlinks, previous), referenced = new Set();

  const allowSlices = supportsImages && stamp.screen === 'primary';
  let cols, rows;
  if (update.kind === 'snapshot') {
    cols = update.cols;
    if (!Number.isInteger(cols) || cols < 1 || cols > 256 || !update.rows.length || cols * update.rows.length > 32768)
      throw new Error('Channel dimension limit');
    if (previous && previous.layout === stamp.layout && (cols !== previous.cols || update.rows.length !== previous.rows.length))
      throw new Error('Channel geometry requires new layout');
    rows = update.rows.map(row => validateRow(row, cols, links, referenced, allowSlices, stamp.version));
  } else {
    if (snapshotOnly || update.kind !== 'delta' || !previous ||
        update.base_revision !== previous.revision || stamp.layout !== previous.layout || stamp.screen !== previous.screen ||
        stamp.version !== previous.version || update.rows.length > previous.rows.length) throw new Error('invalid Channel delta base');
    cols = previous.cols;
    rows = previous.rows.slice();
    const seen = new Set();
    for (const move of moves) {
      const {from, to, count} = move ?? {};
      if (![from,to,count].every(Number.isInteger) || from < 0 || to < 0 || count < 1 || from === to ||
          from + count > rows.length || to + count > rows.length) throw new Error('invalid Channel row move');
      for (let offset = 0; offset < count; offset++) {
        const index = to + offset;
        if (seen.has(index)) throw new Error('overlapping Channel row moves');
        rows[index] = previous.rows[from + offset];
        seen.add(index);
      }
    }
    for (const edit of update.rows) {
      const index = edit?.index;
      if (!Number.isInteger(index) || index < 0 || index >= rows.length || seen.has(index))
        throw new Error('invalid Channel row index');
      rows[index] = validateRow(edit.row, cols, links, referenced, allowSlices, stamp.version);
      seen.add(index);
    }
  }
  if (links.size !== referenced.size) throw new Error('unreferenced Channel hyperlinks');
  if (rows.reduce((bytes, row) => bytes + row.textBytes, 0) > maxTextBytes) throw new Error('Channel text byte limit');
  const hyperlinks = Object.create(null);
  let linkBytes = 0;
  // Unchanged rows keep their previous references. Replacing their last
  // referencing row drops the target from the admitted frame immediately.
  for (const row of rows) for (const id of row.links) {
    if (hyperlinks[id] !== undefined) continue;
    const url = links.get(id) ?? previous?.hyperlinks[id];
    linkBytes += utf8Bytes(url);
    if (linkBytes > maxHyperlinkBytes) throw new Error('Channel hyperlink byte limit');
    hyperlinks[id] = url;
  }
  const {cursor_row, cursor_col, cursor_shape, cursor_visible, pending_wrap, cursor_color} = stamp;
  if (cursor_color !== undefined && (typeof cursor_color !== 'string' || !/^#[a-f0-9]{6}$/i.test(cursor_color)))
    throw new Error('invalid Channel cursor color');
  if (!Number.isInteger(cursor_row) || cursor_row < 0 || cursor_row >= rows.length ||
      !Number.isInteger(cursor_col) || cursor_col < 0 || cursor_col >= cols ||
      !['block','underline','beam'].includes(cursor_shape) || typeof cursor_visible !== 'boolean' || typeof pending_wrap !== 'boolean')
    throw new Error('invalid Channel cursor');
  const images = resolveImages(rows, cols, newImages, imageBase);
  const frame = Object.freeze({version: stamp.version, session: stamp.session, epoch: expected.epoch,
    revision: stamp.revision, layout: stamp.layout, screen: stamp.screen, cols, rows: Object.freeze(rows),
    hyperlinks:Object.freeze(hyperlinks), images, cursor_row, cursor_col, cursor_shape, cursor_visible, pending_wrap, cursor_color});
  if (moves.length === 1) {
    const {from,to,count} = moves[0];
    frameMoves.set(frame,Object.freeze({base:update.base_revision,from,to,count}));
  }
  return frame;
}

export const MAX_PROJECTED_HISTORY_ROWS = 65536;
const maxHistoryTextBytes = 131200;
const historyBlank = Object.freeze({text:' ',foreground:'#d9e5ef',background:'#111820',width:1,style:0});

// The browser selects from the same immutable text that supplied these paint
// rows. A search window can also carry a short prefix of the following window.
export function acceptHistoryProjection(encoded, owner, search = false, expectedVersion) {
  const limit = maxEncodedBytes + maxEncodedImageBytes;
  if (typeof encoded !== 'string' || encoded.length > limit) throw new Error('history projection byte limit');
  const wireBytes = encodedUtf8Bytes(encoded);
  if (wireBytes > limit) throw new Error('history projection byte limit');
  const view = JSON.parse(encoded);
  const packed = supportsPackedRows(view?.version);
  if (!view || (!packed && view.version !== CHANNEL_IMAGE_VERSION) ||
      (expectedVersion !== undefined && view.version !== expectedVersion) || view.owner?.session !== owner.session ||
      view.owner.epoch !== owner.epoch || view.owner.generation !== owner.generation)
    throw new Error('foreign history projection');
  const {cols,text,offset,total_rows,revision,default_background} = view;
  if (default_background !== undefined && (typeof default_background !== 'string' || !/^#[a-f0-9]{6}$/i.test(default_background)))
    throw new Error('invalid history default background');
  const blank = default_background === undefined || default_background === historyBlank.background
    ? historyBlank : Object.freeze({...historyBlank,background:default_background});
  channelCounter(revision);
  if (!Number.isInteger(cols) || cols < 1 || cols > 256 ||
      !Number.isInteger(offset) || offset < 0 || !Number.isInteger(total_rows) ||
      total_rows < offset || total_rows > MAX_PROJECTED_HISTORY_ROWS ||
      !Array.isArray(view.rows) || view.rows.length > 128 || view.rows.length + offset > total_rows ||
      (total_rows > 0 && view.rows.length === 0) ||
      typeof text !== 'string' || text.length > maxHistoryTextBytes || utf8Bytes(text) > maxHistoryTextBytes ||
      /[\x00-\x08\x0b-\x1f\x7f-\x9f\ud800-\udfff]/u.test(text))
    throw new Error('invalid history projection bounds');
  const {images: newImages, encodedBytes} = validatedImages(view.images,true,null,null);
  if (wireBytes > maxEncodedBytes + encodedBytes) throw new Error('history projection byte limit');
  const links = validatedHyperlinks(view.hyperlinks,null), referenced = new Set();
  const sources = [], offsets = [];
  let previousRow = 0n, previousColumn = -1, end = 0, wraps = true, paintBytes = 0;
  const rows = view.rows.map(entry => {
    const source = entry?.source;
    const rowId = channelCounter(source?.row);
    if (!Number.isInteger(source?.column) || source.column < 0 || source.column >= 256 ||
        rowId < previousRow || (rowId === previousRow && source.column <= previousColumn))
      throw new Error('invalid history row source');
    previousRow = rowId;previousColumn = source.column;
    sources.push(Object.freeze({row:source.row,column:source.column}));
    const width = packed && entry.paint?.cells === undefined ? entry.paint?.columns : entry.paint?.cells?.length;
    if (!Number.isInteger(width) || width < 0 || width > cols) throw new Error('invalid history row width');
    const row = validateRow(entry.paint,width,links,referenced,true,view.version);
    paintBytes += row.textBytes;
    const bounds = entry.text_offsets;
    if (!Array.isArray(bounds) || bounds.length !== width + 1 || bounds[0] !== end + Number(!wraps) ||
        (!wraps && text[end] !== '\n')) throw new Error('invalid history copy boundary');
    for (let i = 0; i < width; i++) {
      const first = bounds[i], last = bounds[i + 1], {text: glyph, width: span} = row[i];
      if (!Number.isInteger(last) || last < first || last > text.length ||
          (last === first ? span !== 0 && glyph !== ' ' : span === 0 || text.slice(first,last) !== glyph))
        throw new Error('invalid history copy range');
    }
    end = bounds[width];wraps = row.wrapped;
    offsets.push(Object.freeze(bounds));
    return Object.freeze(Object.assign([...row,...Array(cols-width).fill(blank)], {imageSlices:row.imageSlices}));
  });
  if (end > text.length || text.length - end > (search ? 128 : 0) ||
      paintBytes > maxTextBytes || links.size !== referenced.size)
    throw new Error('invalid history projection resources');
  return Object.freeze({session:owner.session,epoch:owner.epoch,revision,offset,total_rows,text,cols,
    rows:Object.freeze(rows), sources:Object.freeze(sources), offsets:Object.freeze(offsets),
    images:resolveImages(rows,cols,newImages,null,true),cursor_visible:false});
}
