export function packedRow({wrapped, cells, image_slices}) {
  const styles = [], runs = [], indices = new Map();
  for (const {text, width, ...paint} of cells) {
    const key = JSON.stringify(paint);
    if (!indices.has(key)) {
      indices.set(key, styles.length);
      styles.push(paint);
    }
    const index = indices.get(key), last = runs.at(-1);
    if (width === 1 && [...text].length === 1) {
      if (last?.length === 2 && last[0] === index) last[1] += text;
      else runs.push([index, text]);
    } else runs.push([index, text, width]);
  }
  return {wrapped,columns:cells.length,styles,runs,image_slices};
}
export function binaryChannelEvent(metadata, ...pixels) {
  const json = new TextEncoder().encode(JSON.stringify(metadata));
  const rasterBytes = pixels.reduce((length,pixels)=>length+pixels.length,0);
  const bytes = new Uint8Array(rasterBytes+json.length+8);
  let offset = 0;
  for (const raster of pixels) {
    bytes.set(raster,offset);
    offset += raster.length;
  }
  bytes.set(json,rasterBytes);
  const footer = new DataView(bytes.buffer);
  footer.setUint32(bytes.length-8,json.length,true);
  footer.setUint32(bytes.length-4,0x3152544a,true);
  return bytes.buffer;
}
