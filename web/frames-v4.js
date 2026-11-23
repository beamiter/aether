export const CHANNEL_FRAME_VERSION = 5;
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

function validatedImages(entries, supportsImages, base, previous) {
  const images = new Map();
  if (entries === undefined) return {images, encodedBytes: 0};
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
    bytes += size;
    if (bytes > maxImageBytes || typeof rgba !== 'string' || rgba.length !== 4 * Math.ceil(size / 3))
      throw new Error('Channel image byte limit');
    const padding = (3 - size % 3) % 3;
    const end = rgba.length - padding;
    const last = base64Alphabet.indexOf(rgba[end - 1]);
    if (/[^a-zA-Z0-9+/]/.test(rgba.slice(0,end)) || rgba.slice(end) !== '='.repeat(padding) ||
        (padding && (last & (padding === 1 ? 3 : 15)))) throw new Error('invalid Channel RGBA base64');
    encodedBytes += rgba.length;
    const prior = previous?.images[id];
    if (prior && (prior.width !== width || prior.height !== height || prior.columns !== columns ||
        prior.rows !== rows || prior.rgba !== rgba)) throw new Error('changed Channel image identity');
    images.set(id,prior ?? Object.freeze({id,width,height,columns,rows,rgba,bytes:size}));
  }
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

function resolveImages(rows, cols, newImages, base) {
  const images = Object.create(null);
  const referenced = new Set();
  let bytes = 0, count = 0;
  for (const row of rows) {
    for (const {id,column,slice} of row.imageSlices ?? []) {
      const resource = newImages.get(id) ?? base?.images[id];
      if (!resource || slice >= resource.rows || column + resource.columns > cols)
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

function validateRow(row, cols, links, referenced, supportsImages) {
  if (!row || typeof row.wrapped !== 'boolean' || !Array.isArray(row.cells) || row.cells.length !== cols)
    throw new Error('invalid Channel row');
  let textBytes = 0;
  const rowLinks = new Set();
  const cells = row.cells.map((cell, i) => {
    if (!cell || !Number.isInteger(cell.width) || cell.width < 0 || cell.width > 2 ||
        !Number.isInteger(cell.style) || cell.style < 0 || cell.style > 123 || (cell.style & 4) !== 0 ||
        typeof cell.text !== 'string' || cell.text.length === 0 || cell.text.length > maxClusterBytes ||
        /[\x00-\x1f\x7f-\x9f\ud800-\udfff]/u.test(cell.text) ||
        typeof cell.foreground !== 'string' || !colors.test(cell.foreground) ||
        typeof cell.background !== 'string' || !colors.test(cell.background) ||
        (cell.underline !== undefined && !underlines.has(cell.underline)) ||
        (cell.underline_color !== undefined && (typeof cell.underline_color !== 'string' || !colors.test(cell.underline_color))) ||
        (cell.hyperlink_id !== undefined && !links.has(cell.hyperlink_id)))
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
    return Object.freeze({text, foreground, background, width, style, underline, underline_color, hyperlink_id});
  });
  const imageSlices = validateSlices(row.image_slices, cols, supportsImages);
  return Object.freeze(Object.assign(cells, {wrapped: row.wrapped, textBytes, links:Object.freeze([...rowLinks]), imageSlices}));
}

export function acceptChannelFrame(previous, encoded, expected, snapshotOnly = false) {
  const expectedVersion = expected.frameVersion ?? previous?.version;
  const limit = maxEncodedBytes + (expectedVersion === CHANNEL_FRAME_VERSION ? maxEncodedImageBytes : 0);
  if (typeof encoded !== 'string' || encoded.length > limit) throw new Error('Channel frame byte limit');
  const wireBytes = utf8Bytes(encoded);
  if (wireBytes > limit) throw new Error('Channel frame byte limit');
  const update = JSON.parse(encoded);
  const stamp = update?.stamp;
  if (!stamp || ![textChannelVersion,CHANNEL_FRAME_VERSION].includes(stamp.version) ||
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
  const supportsImages = stamp.version === CHANNEL_FRAME_VERSION;
  const imageBase = update.kind === 'delta' ? previous : null;
  const {images: newImages, encodedBytes} = validatedImages(update.images, supportsImages && stamp.screen === 'primary', imageBase, previous);
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
    rows = update.rows.map(row => validateRow(row, cols, links, referenced, allowSlices));
  } else {
    if (snapshotOnly || update.kind !== 'delta' || !previous ||
        update.base_revision !== previous.revision || stamp.layout !== previous.layout || stamp.screen !== previous.screen ||
        update.rows.length > previous.rows.length) throw new Error('invalid Channel delta base');
    cols = previous.cols;
    rows = previous.rows.slice();
    const seen = new Set();
    for (const edit of update.rows) {
      const index = edit?.index;
      if (!Number.isInteger(index) || index < 0 || index >= rows.length || seen.has(index))
        throw new Error('invalid Channel row index');
      rows[index] = validateRow(edit.row, cols, links, referenced, allowSlices);
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
  const {cursor_row, cursor_col, cursor_shape, cursor_visible, pending_wrap} = stamp;
  if (!Number.isInteger(cursor_row) || cursor_row < 0 || cursor_row >= rows.length ||
      !Number.isInteger(cursor_col) || cursor_col < 0 || cursor_col >= cols ||
      !['block','underline','beam'].includes(cursor_shape) || typeof cursor_visible !== 'boolean' || typeof pending_wrap !== 'boolean')
    throw new Error('invalid Channel cursor');
  const images = resolveImages(rows, cols, newImages, imageBase);
  return Object.freeze({version: stamp.version, session: stamp.session, epoch: expected.epoch,
    revision: stamp.revision, layout: stamp.layout, screen: stamp.screen, cols, rows: Object.freeze(rows),
    hyperlinks:Object.freeze(hyperlinks), images, cursor_row, cursor_col, cursor_shape, cursor_visible, pending_wrap});
}
