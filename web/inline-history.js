import {acceptHistoryProjection, CHANNEL_RENDITION_VERSION} from './frames-v4.js';
import {CELL_HEIGHT, clearPaint, paint} from './frames.js';

// Keep the PTY frame live underneath the reader. History requests neither
// acknowledge that frame nor send input to the foreground application.
export function installInlineHistory({canvas, viewport, getOwner, getFrame, fetchHistory, focus}) {
  let owner = null;
  let oldest;
  let reader = null;
  let wheelRemainder = 0;

  function reset() {
    reader = null;
    wheelRemainder = 0;
    if (!canvas.hidden) clearPaint(canvas);
    canvas.hidden = true;
    canvas.style.display = 'none';
  }

  async function render(current) {
    if (current.loading) return;
    current.loading = true;
    const isCurrent = () => reader === current && getOwner() === current.owner;
    try {
      const frame = current.frame;
      const height = frame.rows.length;
      while (isCurrent()) {
        const page = current.page;
        if (page) {
          current.distance = Math.min(current.distance, current.edge);
          if (!current.distance) {
            reset();
            return;
          }
          const top = current.edge - current.distance;
          const first = page.total_rows - page.offset - page.rows.length;
          const end = Math.min(current.edge, top + height);
          if (top >= first && top < first + page.rows.length &&
              (end <= first + page.rows.length || current.requestedTop === top)) {
            const archived = page.rows.slice(top - first, end - first);
            const rows = [...archived, ...frame.rows.slice(0, height - archived.length)];
            // Retained rows may still use the width at which they were written.
            canvas.style.backgroundColor = frame.rows[0][0].background;
            paint(canvas, {...frame, rows, cursor_visible: false, selection: undefined,
              images: {...frame.images, ...page.images}});
            canvas.hidden = false;
            canvas.style.display = 'block';
            return;
          }
          if (current.requestedTop === top) {
            reset();
            return;
          }
          current.requestedTop = top;
        }
        const anchor = page?.sources[0];
        const first = page ? page.total_rows - page.offset - page.rows.length : 0;
        const position = anchor ? {anchor, shift: current.requestedTop - first} : {};
        const value = await fetchHistory(current.owner, {
          frameVersion: CHANNEL_RENDITION_VERSION, offset: 0, ...position,
        });
        if (!isCurrent()) return;
        const next = acceptHistoryProjection(value, current.owner, false, CHANNEL_RENDITION_VERSION);
        if (!next.rows.length) {
          reset();
          return;
        }
        current.edge ??= next.total_rows;
        current.page = next;
      }
    } catch {
      if (isCurrent()) reset();
    } finally {
      current.loading = false;
    }
  }

  function ready() {
    const frame = getFrame();
    return owner !== null && owner === getOwner() &&
      frame?.session === owner.session && frame.epoch === owner.epoch;
  }

  function scroll(rows) {
    if (!ready()) return false;
    if (!rows) return true;
    if (!reader) {
      if (rows > 0) return true;
      reader = {owner, frame: getFrame(), distance: 0, loading: false};
    }
    reader.distance = Math.max(0, Math.min(reader.edge ?? Number.MAX_SAFE_INTEGER, reader.distance - rows));
    if (!reader.distance) reset();
    else void render(reader);
    return true;
  }

  const pageRows = () => Math.max(1, Math.min(getFrame()?.rows.length ?? 1,
    Math.floor(viewport.clientHeight / CELL_HEIGHT) || 1) - 1);
  viewport.addEventListener('wheel', event => {
    if (!ready() || event.defaultPrevented || event.ctrlKey || event.altKey || event.metaKey || event.shiftKey ||
        !Number.isFinite(event.deltaY) || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return;
    const delta = event.deltaY * (event.deltaMode === 1 ? 3 : event.deltaMode === 2 ? pageRows() : 1 / CELL_HEIGHT);
    if (wheelRemainder * delta < 0) wheelRemainder = 0;
    const rows = Math.trunc(wheelRemainder + delta);
    wheelRemainder += delta - rows;
    if (scroll(rows)) event.preventDefault();
  }, {capture: true, passive: false});
  canvas.addEventListener('pointerdown', event => {
    event.preventDefault();
    focus();
  });
  reset();

  return {
    active: () => reader !== null,
    reset,
    update(attachment, enabled, firstRetained) {
      const next = enabled ? attachment : null;
      if (owner !== next || (firstRetained !== undefined && oldest !== firstRetained) ||
          (reader && reader.frame.layout !== getFrame()?.layout)) reset();
      if (firstRetained !== undefined) oldest = firstRetained;
      owner = next;
    },
    page(event) {
      if (event.shiftKey || event.altKey || event.ctrlKey || event.metaKey ||
          (event.key !== 'PageUp' && event.key !== 'PageDown')) return false;
      return scroll(event.key === 'PageUp' ? -pageRows() : pageRows());
    },
  };
}
