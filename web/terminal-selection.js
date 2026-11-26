import {CELL_HEIGHT, CELL_WIDTH} from './frames.js';

function sameSurface(a, b) {
  return a?.session === b.session && a.epoch === b.epoch &&
    a.layout === b.layout && a.screen === b.screen &&
    a.cols === b.cols && a.rows.length === b.rows.length;
}

function cellAt(frame, canvas, event, clamp) {
  const rect = canvas.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0 ||
      !Number.isFinite(event.clientX) || !Number.isFinite(event.clientY)) return null;

  let col = Math.floor((event.clientX - rect.left) * frame.cols / rect.width);
  let row = Math.floor((event.clientY - rect.top) * frame.rows.length / rect.height);
  if (clamp) {
    col = Math.max(0, Math.min(frame.cols - 1, col));
    row = Math.max(0, Math.min(frame.rows.length - 1, row));
  }
  const cell = frame.rows[row]?.[col];
  if (!cell) return null;
  return row * frame.cols + col - Number(cell.width === 0);
}

function selectionRows(frame, range) {
  const first = Math.min(range.anchor, range.active);
  const last = Math.max(range.anchor, range.active);
  const top = Math.floor(first / frame.cols);
  const bottom = Math.floor(last / frame.cols);
  const left = first % frame.cols;
  const lastCol = last % frame.cols;
  const right = lastCol + frame.rows[bottom][lastCol].width;
  return {top, bottom, left, right};
}

function selectedText(frame, range) {
  const {top, bottom, left, right} = selectionRows(frame, range);
  let text = '';
  for (let row = top; row <= bottom; row++) {
    if (row > top && !frame.row_wrapped[row - 1]) text += '\n';
    const cells = frame.rows[row].slice(row === top ? left : 0, row === bottom ? right : frame.cols);
    for (const cell of cells) {
      if (cell.width !== 0) text += cell.text;
    }
  }
  return text;
}

export function installTerminalSelection({canvas, overlay, document, onFrame, onSelection}) {
  let latest = null;
  let visible = null;
  let drag = null;
  let selection = null;
  let suppressClick = false;

  function drawSelection() {
    const range = drag?.moved ? drag.range : selection?.range;
    if (!range) {
      overlay.replaceChildren();
      return;
    }
    const {top, bottom, left, right} = selectionRows(visible, range);
    const rectangles = [];
    function add(row, col, rows, cols) {
      const rectangle = document.createElement('span');
      rectangle.style.left = `${col * CELL_WIDTH}px`;
      rectangle.style.top = `${row * CELL_HEIGHT}px`;
      rectangle.style.width = `${cols * CELL_WIDTH}px`;
      rectangle.style.height = `${rows * CELL_HEIGHT}px`;
      rectangles.push(rectangle);
    }
    if (top === bottom) {
      add(top, left, 1, right - left);
    } else {
      add(top, left, 1, visible.cols - left);
      if (bottom > top + 1) add(top + 1, 0, bottom - top - 1, visible.cols);
      add(bottom, 0, 1, right);
    }
    overlay.replaceChildren(...rectangles);
  }

  function showLatest() {
    if (latest !== visible) {
      if (selection?.range && (!latest || !sameSurface(visible, latest) ||
          selectedText(latest, selection.range) !== selection.text)) selection.range = null;
      visible = latest;
      onFrame(visible);
    }
    drawSelection();
  }

  function finish(keep, cancelClick = false) {
    if (!drag) return;
    const {pointerId, range, moved} = drag;
    drag = null;
    suppressClick = moved || cancelClick;
    if (canvas.hasPointerCapture(pointerId)) canvas.releasePointerCapture(pointerId);
    if (keep && moved) {
      selection = {range, text: selectedText(visible, range), revision: visible.revision};
      onSelection(true);
    }
    showLatest();
  }

  canvas.addEventListener('pointerdown', event => {
    if (drag || !event.isPrimary || event.button !== 0) return;
    suppressClick = false;
    if (!visible || event.defaultPrevented || event.pointerType !== 'mouse' ||
        event.ctrlKey || event.altKey || event.metaKey ||
        (event.shiftKey && (!visible.mouse || visible.mouse.tracking === 'off'))) return;
    const anchor = cellAt(visible, canvas, event, false);
    if (anchor === null) return;
    canvas.setPointerCapture(event.pointerId);
    event.preventDefault();
    drag = {
      pointerId: event.pointerId,
      x: event.clientX,
      y: event.clientY,
      range: {anchor, active: anchor},
      moved: false,
    };
    selection = null;
    onSelection(false);
    drawSelection();
  });
  function update(event) {
    if (drag?.pointerId !== event.pointerId) return;
    const active = cellAt(visible, canvas, event, true);
    if (active === null) return;
    const changed = active !== drag.range.active;
    const moved = changed || Math.hypot(event.clientX - drag.x, event.clientY - drag.y) >= 3;
    if (!changed && (drag.moved || !moved)) return;
    drag.range.active = active;
    drag.moved = true;
    drawSelection();
  }
  canvas.addEventListener('pointermove', update);
  canvas.addEventListener('pointerup', event => {
    if (drag?.pointerId !== event.pointerId || event.button !== 0) return;
    update(event);
    finish(true);
  });
  for (const name of ['pointercancel', 'lostpointercapture']) {
    canvas.addEventListener(name, event => {
      if (drag?.pointerId === event.pointerId) finish(false, true);
    });
  }
  canvas.addEventListener('click', event => {
    if (!suppressClick) return;
    suppressClick = false;
    event.preventDefault();
  }, {capture: true});
  globalThis.addEventListener?.('blur', () => finish(false, true));

  return {
    present(frame) {
      latest = frame;
      if (drag && sameSurface(visible, frame)) return;
      if (drag) finish(false, true);
      else showLatest();
    },
    visibleFrame: () => visible,
    snapshot: () => selection && {text: selection.text, revision: selection.revision},
    cancelDrag: () => finish(false, true),
    reset() {
      latest = null;
      if (drag) finish(false, true);
      selection = null;
      onSelection(false);
      showLatest();
    },
  };
}
