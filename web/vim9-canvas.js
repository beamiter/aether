// This layer paints Vim-owned buffer runs over the live terminal surface.
// Scrollbars, status rows and fallback painting belong to the PTY view.
const MAX_BACKING_PIXELS = 16 * 1024 * 1024;
const rgb = (color, fallback) => Number.isInteger(color) && color >= 0 && color <= 0xffffff
  ? `#${color.toString(16).padStart(6, '0')}` : fallback;
const integerAtLeast = (value, minimum) => Number.isSafeInteger(value) && value >= minimum;

export function canPaintVim9(frame, metrics, terminal) {
  if (!frame?.supported || !metrics || !terminal || frame.rows !== terminal.rows.length ||
      frame.columns !== terminal.cols || !integerAtLeast(frame.revision, 1) || frame.completion ||
      !Array.isArray(frame.popups) || frame.popups.length || !Array.isArray(frame.windows) ||
      !Array.isArray(frame.runs) || frame.runs.length > 32768 ||
      ![metrics.origin_x, metrics.origin_y, metrics.cell_width, metrics.line_height].every(Number.isFinite) ||
      metrics.origin_x < 0 || metrics.origin_y < 0 || metrics.cell_width <= 0 || metrics.cell_width > 64 ||
      metrics.line_height <= 0 || metrics.line_height > 64) return false;
  const views = frame.windows.length ? frame.windows.map(window => window.view) : [frame.view];
  return views.every(rect => rect && integerAtLeast(rect.row, 0) && integerAtLeast(rect.column, 0) &&
    integerAtLeast(rect.rows, 1) && integerAtLeast(rect.columns, 1) &&
    rect.row + rect.rows <= frame.rows && rect.column + rect.columns <= frame.columns);
}

function colors(style, inherit) {
  const foreground = rgb(style.foreground, inherit.foreground);
  const background = rgb(style.background, inherit.background);
  return style.reverse ? {foreground: background, background: foreground} : {foreground, background};
}

export function paintVim9(canvas, frame, metrics, dpr = globalThis.devicePixelRatio || 1) {
  const {origin_x: left, origin_y: top, cell_width: cell, line_height: line} = metrics;
  const width = left + frame.columns * cell, height = top + frame.rows * line;
  const scale = Math.min(Number.isFinite(dpr) && dpr > 0 ? dpr : 1, 4, Math.sqrt(MAX_BACKING_PIXELS / (width * height)));
  canvas.width = Math.max(1, Math.floor(width * scale));
  canvas.height = Math.max(1, Math.floor(height * scale));
  canvas.style.width = `${width}px`;
  canvas.style.height = `${height}px`;
  const context = canvas.getContext('2d');
  if (!context) throw Error('Vim buffer canvas unavailable.');
  context.setTransform(scale, 0, 0, scale, 0, 0);
  const base = colors(frame.default_style || {}, {foreground: '#d7e0e9', background: '#131a21'});
  const rectangles = frame.windows.length ? frame.windows.map(window => window.view) : [frame.view];
  const x = column => left + column * cell;
  const y = row => top + row * line;
  context.save();
  context.beginPath();
  for (const view of rectangles) context.rect(x(view.column), y(view.row), view.columns * cell, view.rows * line);
  context.clip();
  context.fillStyle = base.background;
  context.fillRect(0, 0, width, height);
  context.textBaseline = 'middle';
  for (const run of frame.runs) {
    const style = run.style || {};
    const palette = colors(style, base);
    const startX = x(run.column), startY = y(run.row), runWidth = run.cells * cell;
    context.fillStyle = palette.background;
    context.fillRect(startX, startY, runWidth, line);
    context.fillStyle = palette.foreground;
    context.font = `${style.italic ? 'italic ' : ''}${style.bold ? 'bold ' : ''}${line * .75}px monospace`;
    if (run.text.length === run.cells && /^[\x20-\x7e]+$/.test(run.text)) {
      for (let index = 0; index < run.cells; index++) {
        context.fillText(run.text[index], startX + index * cell, startY + line / 2, cell);
      }
    } else {
      context.fillText(run.text, startX, startY + line / 2, runWidth);
    }
    const lineWidth = Math.max(1 / scale, 1);
    if (style.underline || style.undercurl) {
      context.fillStyle = rgb(style.special, palette.foreground);
      context.fillRect(startX, startY + line - 3, runWidth, lineWidth);
    }
    if (style.strikethrough) {
      context.fillStyle = palette.foreground;
      context.fillRect(startX, startY + line / 2, runWidth, lineWidth);
    }
  }
  const cursor = frame.cursor;
  if (cursor?.visible) {
    const startX = x(cursor.column), startY = y(cursor.row), cursorWidth = Math.max(1, cursor.width) * cell;
    const appearance = cursor.appearance;
    const color = rgb(appearance?.style?.background, rgb(appearance?.style?.foreground, base.foreground));
    const fraction = Math.max(.01, Math.min(1, (appearance?.percentage ?? 100) / 100));
    context.fillStyle = context.strokeStyle = color;
    if (appearance?.shape === 'vertical') {
      context.fillRect(startX, startY, Math.max(1 / scale, cell * fraction), line);
    } else if (appearance?.shape === 'horizontal') {
      const thickness = Math.max(1 / scale, line * fraction);
      context.fillRect(startX, startY + line - thickness, cursorWidth, thickness);
    } else {
      context.strokeRect(startX + .5, startY + .5, cursorWidth - 1, line - 1);
    }
  }
  context.restore();
}
