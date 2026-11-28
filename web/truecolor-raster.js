import {rgbValue} from './cell-colors.js';
const FIRST_GLYPH = 32, LAST_GLYPH = 126;
const GLYPHS = LAST_GLYPH - FIRST_GLYPH + 1, LEVELS = 8;
const ATLAS_COLUMNS = 16, PROBES = 48;
const MIN_DAMAGE = 1536, SAMPLE_CELLS = 256;
const BAND_PIXELS = 256 * 1024;
const MASK_PITCHES = 3;
const redBlueLanes = 0xff00ff, greenLane = 0x00ff00;
const littleEndian = new Uint8Array(Uint32Array.of(1).buffer)[0] === 1;

export function canRasterizeInk(cell) {
  return cell.width === 1 && cell.style === 0 && !cell.underline && !cell.hyperlink_id &&
    cell.text.length === 1 && cell.text >= ' ' && cell.text <= '~';
}

function rgba(r,g,b) {
  return littleEndian ? (0xff000000 | b<<16 | g<<8 | r) >>> 0 : (r<<24 | g<<16 | b<<8 | 255) >>> 0;
}

// Skia's grayscale masks use luminance after expanding the high three bits of
// each channel. The pixel probe below checks both this mapping and the blend.
function quantize(channel) {
  const high = channel & 224;
  return high | high>>3 | high>>6;
}
function maskIndex(foreground, code) {
  const light = (54*quantize(foreground>>>16) + 183*quantize(foreground>>8) + 19*quantize(foreground)) >> 13;
  return light * GLYPHS + code - FIRST_GLYPH;
}
function blend(foreground, background, alpha) {
  const a = alpha+1, b = 256-alpha;
  const r = ((foreground>>>16)*a>>8) + ((background>>>16)*b>>8);
  const g = ((foreground>>8&255)*a>>8) + ((background>>8&255)*b>>8);
  const blue = ((foreground&255)*a>>8) + ((background&255)*b>>8);
  return rgba(r,g,blue);
}

function nativeRgb(rgb) {
  return littleEndian ? (rgb>>>16) | (rgb&greenLane) | ((rgb&255)<<16) : rgb;
}

function opaqueRgb(rgb) {
  return littleEndian ? rgb | 0xff000000 : (rgb<<8) | 255;
}

function blendRgbLanes(fgRB, bgRB, fgGreen, bgGreen, alpha) {
  const a=alpha+1, b=256-alpha;
  // The gap between red and blue keeps their 16-bit products independent.
  // Truncate source and destination separately, as in the calibration blend.
  const rb=((Math.imul(fgRB,a)>>>8)&redBlueLanes)+((Math.imul(bgRB,b)>>>8)&redBlueLanes);
  const green=((Math.imul(fgGreen,a)>>>8)&greenLane)+((Math.imul(bgGreen,b)>>>8)&greenLane);
  return opaqueRgb(rb|green);
}

function wantsRaster(frame, damage) {
  let cells = 0;
  for (const {y,first,last} of damage) {
    if (last > frame.rows[y].length) return false;
    cells += last-first;
  }
  if (cells < MIN_DAMAGE || cells < damage.length*12) return false;
  let checked = 0, compatible = 0, ink = 0;
  const pairs = new Set();
  for (const {y,first,last} of damage) {
    for (let x=first; x<last; x++) {
      const cell = frame.rows[y][x];
      if (canRasterizeInk(cell)) {
        compatible++;
        if (cell.text !== ' ') ink++;
      }
      pairs.add(rgbValue(cell.foreground)*0x1000000+rgbValue(cell.background));
      if (++checked === SAMPLE_CELLS) return compatible*5 >= checked*4 && ink >= 64 && pairs.size >= 64;
    }
  }
  return false;
}

function clippedGlyph(context, text, x, y, width, height, textTop) {
  context.save();
  try {
    context.beginPath();
    context.rect(x,y,width,height);
    context.clip();
    context.fillText(text,x,y+textTop);
  } finally {
    context.restore();
  }
}

function readMasks(document, {language,font,scale}, cellWidth, cellHeight, textTop) {
  const surface = document.createElement('canvas');
  const columns = ATLAS_COLUMNS, maskCount = GLYPHS*LEVELS;
  const width = cellWidth*scale, height = cellHeight*scale;
  surface.lang = language;
  surface.width = columns*width;
  surface.height = Math.ceil((maskCount+PROBES)/columns)*height;
  const g = surface.getContext('2d',{willReadFrequently:true});
  if (!g) return null;
  g.setTransform(scale,0,0,scale,0,0);
  g.direction = 'ltr';
  g.textAlign = 'start';
  g.textBaseline = 'top';
  g.font = font;
  for (let slot=0; slot<maskCount; slot++) {
    const value = Math.floor(slot/GLYPHS)*32;
    const code = FIRST_GLYPH + slot%GLYPHS;
    if (code === FIRST_GLYPH) g.fillStyle = '#' + value.toString(16).padStart(2,'0').repeat(3);
    else clippedGlyph(g,String.fromCharCode(code),slot%columns*cellWidth,
      Math.floor(slot/columns)*cellHeight,cellWidth,cellHeight,textTop);
  }

  const probes = [];
  for (let i=0; i<PROBES; i++) {
    const slot = maskCount+i, text = 'Wj%gm/'[i%6];
    const foreground = Math.imul(i+31,0x9e3779b1)>>>8;
    const background = Math.imul(i+17,0x4f1bbcdc)>>>8;
    const x=slot%columns*cellWidth, y=Math.floor(slot/columns)*cellHeight;
    g.fillStyle = '#' + background.toString(16).padStart(6,'0');
    g.fillRect(x,y,cellWidth,cellHeight);
    g.fillStyle = '#' + foreground.toString(16).padStart(6,'0');
    clippedGlyph(g,text,x,y,cellWidth,cellHeight,textTop);
    probes.push({slot,foreground,background,mask:maskIndex(foreground,text.charCodeAt(0))});
  }
  let image;
  try {
    image = g.getImageData(0,0,surface.width,surface.height);
  } catch {
    return null;
  }
  const pixels = new Uint32Array(image.data.buffer,image.data.byteOffset,image.data.length/4);
  const offset = slot => Math.floor(slot/columns)*height*surface.width + slot%columns*width;
  // Only use the software path if the live browser produces the same pixels
  // for representative foregrounds, backgrounds and partially covered glyphs.
  let partial = 0;
  for (const {slot,foreground,background,mask} of probes) {
    const maskStart = offset(mask), probeStart = offset(slot);
    for (let y=0; y<height; y++) for (let x=0; x<width; x++) {
      const delta = y*surface.width+x;
      const alpha = image.data[(maskStart+delta)*4+3];
      if (alpha > 0 && alpha < 255) partial++;
      if (blend(foreground,background,alpha) !== pixels[probeStart+delta]) return null;
    }
  }
  if (!partial) return null;
  const masks = [];
  for (let slot=0; slot<maskCount; slot++) {
    const start = offset(slot), points = [];
    for (let y=0; y<height; y++) for (let x=0; x<width; x++) {
      const alpha = image.data[(start+y*surface.width+x)*4+3];
      if (alpha) points.push(y,x,alpha);
    }
    masks.push(Uint8Array.from(points));
  }
  return masks;
}

function masksForPitch(atlas, width) {
  let packed = atlas.pitches.get(width);
  if (packed) atlas.pitches.delete(width);
  else {
    packed = atlas.masks.map(points=>{
      let opaque = 0;
      for (let i=2; i<points.length; i+=3) if (points[i] === 255) opaque++;
      const mask = new Uint32Array(points.length/3+1);
      const firstPartial = opaque+1;
      mask[0] = firstPartial;
      // Opaque pixel offsets come first. The partial samples carry coverage
      // in their low byte and the pixel offset in the remaining bits.
      for (let i=0,solid=1,partial=firstPartial; i<points.length; i+=3) {
        const offset = points[i]*width+points[i+1], alpha = points[i+2];
        if (alpha === 255) mask[solid++] = offset;
        else mask[partial++] = (offset<<8)|alpha;
      }
      return mask;
    });
    if (atlas.pitches.size === MASK_PITCHES) atlas.pitches.delete(atlas.pitches.keys().next().value);
  }
  atlas.pitches.set(width,packed);
  return packed;
}

export function createTruecolorPainter(cellWidth, cellHeight, textTop) {
  const documents = new WeakMap();
  return (canvas, g, frame, damage, scale) => {
    const document = canvas.ownerDocument;
    if (!document || scale > 2 || typeof g.putImageData !== 'function' || typeof g.createImageData !== 'function' || g.globalAlpha !== 1 ||
        g.globalCompositeOperation !== 'source-over' || g.direction !== 'ltr' || g.textAlign !== 'start' ||
        document.fonts?.status === 'loading' || !wantsRaster(frame,damage)) return false;
    const colorSpace = g.getContextAttributes?.().colorSpace;
    if (colorSpace && colorSpace !== 'srgb') return false;
    let cache = documents.get(document);
    if (!cache) {
      cache = {};
      documents.set(document,cache);
      document.fonts?.addEventListener('loadingdone',() => { cache.atlas = undefined; });
    }
    const language = canvas.closest?.('[lang]')?.lang ?? document.documentElement?.lang ?? '';
    const font = g.font;
    if (cache.atlas?.language !== language || cache.atlas.font !== font || cache.atlas.scale !== scale) {
      const attrs = {language,font,scale};
      cache.atlas = {...attrs,masks:readMasks(document,attrs,cellWidth,cellHeight,textTop),pitches:new Map()};
    }
    if (!cache.atlas.masks) return false;

    const cw = cellWidth*scale, ch = cellHeight*scale, width = frame.cols*cw;
    const masks = masksForPitch(cache.atlas,width);
    const rowsPerBand = Math.max(1,Math.floor(BAND_PIXELS/(width*ch)));
    const height = Math.min(rowsPerBand,frame.rows.length)*ch;
    if (cache.band?.data.width !== width || cache.band.data.height !== height) {
      const data = g.createImageData(width,height);
      cache.band = {data,pixels:new Uint32Array(data.data.buffer,data.data.byteOffset,data.data.length/4)};
    }
    const {data,pixels} = cache.band;
    for (let start=0; start<damage.length;) {
      const {y,first,last} = damage[start];
      let end = start+1;
      while (end < damage.length && end-start < rowsPerBand && damage[end].y === y+end-start &&
          damage[end].first === first && damage[end].last === last) end++;
      for (let i=start; i<end; i++) {
        const row = frame.rows[damage[i].y], line = (i-start)*ch*width;
        for (let x=first; x<last; x++) {
          const color = rgbValue(row[x].background), left = line+x*cw;
          pixels.fill(rgba(color>>>16,color>>8&255,color&255),left,left+cw);
        }
        for (let copy=1; copy<ch; copy*=2) {
          pixels.copyWithin(line+copy*width,line,line+Math.min(copy,ch-copy)*width);
        }
        for (let x=first; x<last; x++) {
          const cell = row[x];
          if (!canRasterizeInk(cell) || cell.text === ' ') continue;
          const foreground = rgbValue(cell.foreground);
          const points = masks[maskIndex(foreground,cell.text.charCodeAt(0))];
          const left = line+x*cw;
          // Earlier glyphs stay in their own cells, so this pixel still
          // holds the cell's opaque background in native byte order.
          const fg = nativeRgb(foreground), bg = littleEndian ? pixels[left]&0xffffff : pixels[left]>>>8;
          const fgRB=fg&redBlueLanes, bgRB=bg&redBlueLanes, fgGreen=fg&greenLane, bgGreen=bg&greenLane;
          const color = opaqueRgb(fg), partial = points[0];
          for (let p=1; p<partial; p++) pixels[left+points[p]] = color;
          for (let p=partial; p<points.length; p++) {
            const point = points[p];
            pixels[left+(point>>>8)] = blendRgbLanes(fgRB,bgRB,fgGreen,bgGreen,point&255);
          }
        }
      }
      g.putImageData(data,0,y*ch,first*cw,0,(last-first)*cw,(end-start)*ch);
      start = end;
    }
    return true;
  };
}
