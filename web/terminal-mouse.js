const BUTTONS = ['left', 'middle', 'right'];

function sameOwner(left, right) {
  return left && right && left.epoch === right.epoch && left.layout === right.layout &&
    left.mouse.generation === right.mouse.generation;
}

function cellAt(canvas, frame, event, clamp = false) {
  const {left, top, width, height} = canvas.getBoundingClientRect();
  if (!(width > 0 && height > 0) || !Number.isFinite(event.clientX) || !Number.isFinite(event.clientY)) {
    return null;
  }
  const col = Math.floor((event.clientX - left) * frame.cols / width);
  const row = Math.floor((event.clientY - top) * frame.rows.length / height);
  if (clamp) {
    return {col: Math.max(0, Math.min(frame.cols - 1, col)),
      row: Math.max(0, Math.min(frame.rows.length - 1, row))};
  }
  return row >= 0 && row < frame.rows.length && col >= 0 && col < frame.cols ? {row, col} : null;
}

export function installTerminalMouse({canvas, getFrame, getEpoch, send, focus}) {
  let drag = null;
  let suppressClick = false;
  let fastFlight = null;
  let pendingFast = null;
  let recentMotion = null;

  function owner() {
    const frame = getFrame(), epoch = getEpoch();
    if (epoch !== null && frame?.epoch === epoch && frame.session === 'pty-demo' &&
        frame.mouse && frame.mouse.tracking !== 'off') return frame;
    return null;
  }

  function report(frame, kind, point, button, event) {
    return {
      layout: frame.layout, generation: frame.mouse.generation, kind, ...point,
      button, alt: Boolean(event?.altKey), ctrl: Boolean(event?.ctrlKey),
    };
  }

  function enqueueFast(frame, payload) {
    if (fastFlight) {
      if (!pendingFast || pendingFast.payload.kind === 'motion' || payload.kind !== 'motion') {
        pendingFast = {frame, payload};
      }
      return;
    }
    const ticket = {frame};
    fastFlight = ticket;
    const complete = () => {
      if (fastFlight !== ticket) return;
      fastFlight = null;
      const next = pendingFast;
      pendingFast = null;
      if (next && sameOwner(next.frame, owner())) enqueueFast(next.frame, next.payload);
    };
    Promise.resolve(send(payload)).then(complete, complete);
  }

  function clearDrag(event, shouldRelease = true) {
    if (!drag) return;
    const {frame, button, pointerId, point} = drag;
    drag = null;
    pendingFast = null;
    recentMotion = null;
    if (canvas.hasPointerCapture(pointerId)) canvas.releasePointerCapture(pointerId);
    if (shouldRelease && sameOwner(frame, owner())) {
      const last = event && cellAt(canvas, frame, event, true) || point;
      void send(report(frame, 'release', last, button, event));
    }
  }

  function localOverride(event) {
    return event.shiftKey || event.metaKey || event.getModifierState?.('AltGraph');
  }

  canvas.addEventListener('pointerdown', event => {
    if (!drag) suppressClick = false;
    if (event.defaultPrevented || event.pointerType !== 'mouse' || !event.isPrimary || localOverride(event)) return;
    const frame = owner(), button = BUTTONS[event.button];
    if (!frame || !button) return;
    const point = cellAt(canvas, frame, event);
    if (!point) return;
    event.preventDefault();
    suppressClick = true;
    if (drag) return;
    drag = {frame, button, pointerId: event.pointerId, point};
    pendingFast = null;
    recentMotion = null;
    canvas.setPointerCapture(event.pointerId);
    focus();
    void send(report(frame, 'press', point, button, event));
  }, {capture: true});

  canvas.addEventListener('pointermove', event => {
    if (event.defaultPrevented || event.pointerType !== 'mouse' || !event.isPrimary) return;
    const frame = owner();
    if (!frame || (drag && (!sameOwner(drag.frame, frame) || drag.pointerId !== event.pointerId))) return;
    if (!drag && (event.buttons || localOverride(event))) return;
    const point = cellAt(canvas, frame, event, Boolean(drag));
    if (!point) return;
    if (drag) drag.point = point;
    if (frame.mouse.tracking !== 'any' && (frame.mouse.tracking !== 'drag' || !drag)) return;
    const payload = report(frame, 'motion', point, drag?.button ?? null, event);
    if (recentMotion && sameOwner(recentMotion.frame, frame) &&
        ['row', 'col', 'button', 'alt', 'ctrl'].every(key => recentMotion.payload[key] === payload[key])) return;
    recentMotion = {frame, payload};
    enqueueFast(frame, payload);
  }, {capture: true});

  canvas.addEventListener('pointerup', event => {
    if (drag?.pointerId !== event.pointerId) return;
    event.preventDefault();
    clearDrag(event);
  }, {capture: true});
  for (const type of ['pointercancel', 'lostpointercapture']) {
    canvas.addEventListener(type, event => {
      if (drag?.pointerId === event.pointerId) clearDrag(event);
    }, {capture: true});
  }
  globalThis.addEventListener?.('blur', () => clearDrag());

  canvas.addEventListener('wheel', event => {
    if (event.defaultPrevented || localOverride(event) || !Number.isFinite(event.deltaY) ||
        !event.deltaY || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return;
    const frame = owner();
    const point = frame && cellAt(canvas, frame, event);
    if (!point) return;
    event.preventDefault();
    enqueueFast(frame, report(frame, event.deltaY < 0 ? 'wheel_up' : 'wheel_down', point, null, event));
  }, {passive: false});

  for (const type of ['click', 'auxclick', 'contextmenu']) {
    canvas.addEventListener(type, event => {
      if (suppressClick || (owner() && !localOverride(event))) event.preventDefault();
      suppressClick = false;
    }, {capture: true});
  }

  return {
    frameChanged() {
      const frame = owner();
      if (drag && !sameOwner(drag.frame, frame)) clearDrag(null, false);
      if (fastFlight && !sameOwner(fastFlight.frame, frame)) {
        fastFlight = pendingFast = recentMotion = null;
      }
    },
  };
}
