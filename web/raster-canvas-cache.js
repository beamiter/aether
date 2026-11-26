// Shared by the active, split, and history canvases in this WebView.
export function createRasterCanvasCache(byteLimit, entryLimit = 64) {
  const entries = new Map();
  let retainedBytes = 0;
  return (document, resource) => {
    const cached = entries.get(resource);
    if (cached) {
      entries.delete(resource);
      entries.set(resource, cached);
      return cached.canvas;
    }

    // Count decoded pixels, the retained source string, and entry overhead.
    const bytes = resource.width * resource.height * 4 + resource.rgba.length * 2 + 256;
    const retain = bytes <= byteLimit && entryLimit > 0;
    if (retain) {
      while (retainedBytes + bytes > byteLimit || entries.size >= entryLimit) {
        const oldest = entries.keys().next().value;
        const expired = entries.get(oldest);
        entries.delete(oldest);
        retainedBytes -= expired.bytes;
        expired.canvas.width = 0;
      }
    }

    const canvas = document.createElement('canvas');
    canvas.width = resource.width;
    canvas.height = resource.height;
    const context = canvas.getContext('2d');
    const pixels = context.createImageData(resource.width, resource.height);
    const rgba = globalThis.atob(resource.rgba);
    for (let i = 0; i < rgba.length; i++) pixels.data[i] = rgba.charCodeAt(i);
    context.putImageData(pixels, 0, 0);
    if (retain) {
      entries.set(resource, {canvas, bytes});
      retainedBytes += bytes;
    }
    return canvas;
  };
}
