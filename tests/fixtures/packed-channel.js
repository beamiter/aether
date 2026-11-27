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
