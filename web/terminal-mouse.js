const BUTTONS = ['left', 'middle', 'right'];

function sameOwner(left, right) {
  return left && right && left.session === right.session && left.epoch === right.epoch && left.layout === right.layout &&
    left.screen === right.screen && left.attachment === right.attachment && left.mouse.generation === right.mouse.generation &&
    left.mouse.tracking === right.mouse.tracking;
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

export function installTerminalMouse({canvas, getFrame, send, focus, canRelease = () => false}) {
  let drag = null;
  let suppressClick = false;
  let fastFlight = null;
  let pendingMotion = null;
  let pendingWheels = [];
  let wheelFraction = null;
  let recentMotion = null;

  function owner() {
    const frame = getFrame();
    if (frame?.mouse && frame.mouse.tracking !== 'off') return frame;
    return null;
  }

  function pointerOwner() {
    const frame = owner();
    return frame?.mouse.tracking === 'alternate_scroll' ? null : frame;
  }

  function report(frame, kind, point, button, event) {
    return {
      layout: String(frame.layout), revision: String(frame.revision), generation: frame.mouse.generation, kind, ...point,
      button, alt: Boolean(event?.altKey), ctrl: Boolean(event?.ctrlKey),
    };
  }

  function wheelGeometry(frame) {
    const {left, top, width, height} = canvas.getBoundingClientRect();
    if (![left, top, width, height].every(Number.isFinite) || !(width > 0 && height > 0) ||
        !(frame?.cols > 0 && frame.rows?.length > 0) ||
        !Number.isFinite(height / frame.rows.length) || !(height / frame.rows.length > 0) ||
        !Number.isFinite(width / frame.cols) || !(width / frame.cols > 0)) return null;
    return [left, top, width, height, frame.cols, frame.rows.length];
  }

  function sameGeometry(left, right) {
    return left && right && left.every((value, index) => value === right[index]);
  }

  function resetWheelFraction() { wheelFraction = null; }

  function wheelKind(payload) { return payload.kind === 'wheel_up' || payload.kind === 'wheel_down'; }

  function drainFast() {
    if (fastFlight) return;
    const current = owner();
    const geometry = pendingWheels.length && current && wheelGeometry(current);
    while (pendingWheels.length) {
      const next = pendingWheels.shift();
      if (sameOwner(next.frame, current) && sameGeometry(next.geometry, geometry)) {
        dispatchFast(current, {...next.payload, revision: String(current.revision)});
        return;
      }
    }
    const next = pendingMotion;
    pendingMotion = null;
    if (next && sameOwner(next.frame, current)) {
      dispatchFast(current, {...next.payload, revision: String(current.revision)});
    }
  }

  function dispatchFast(frame, payload) {
    const ticket = {frame, payload};
    fastFlight = ticket;
    const complete = () => {
      if (fastFlight !== ticket) return;
      fastFlight = null;
      drainFast();
    };
    let completion;
    try { completion = send(payload); } catch { complete(); return; }
    Promise.resolve(completion).then(complete, complete);
  }

  function enqueueFast(frame, payload) {
    if (fastFlight) {
      pendingMotion = {frame, payload};
      return;
    }
    dispatchFast(frame, payload);
  }

  function enqueueWheels(frame, geometry, payload, count) {
    pendingWheels = pendingWheels.filter(next => sameOwner(next.frame, frame) && sameGeometry(next.geometry, geometry));
    const outstanding = pendingWheels.length + Number(fastFlight && wheelKind(fastFlight.payload));
    const admitted = Math.min(count, 64 - outstanding);
    if (!admitted) return;
    pendingMotion = null; // As before, a wheel takes priority over unaccepted motion.
    const next = {frame, geometry, payload};
    for (let index = 0; index < admitted; index++) pendingWheels.push(next);
    drainFast();
  }

  function clearDrag(event, shouldRelease = true) {
    if (!drag) return;
    const {frame, button, pointerId, point} = drag;
    drag = null;
    pendingMotion = null;
    recentMotion = null;
    if (canvas.hasPointerCapture(pointerId)) canvas.releasePointerCapture(pointerId);
    const current = owner();
    const releaseFrame = sameOwner(frame, current) ? current
      : !current && canRelease(frame) ? frame : null;
    if (shouldRelease && releaseFrame) {
      const last = event && cellAt(canvas, releaseFrame, event, true) || point;
      void send(report(releaseFrame, 'release', last, button, event));
    }
  }

  function localOverride(event) {
    return event.shiftKey || event.metaKey || event.getModifierState?.('AltGraph');
  }

  canvas.addEventListener('pointerdown', event => {
    if (localOverride(event)) resetWheelFraction();
    if (!drag) suppressClick = false;
    if (event.defaultPrevented || event.pointerType !== 'mouse' || !event.isPrimary || localOverride(event)) return;
    const frame = pointerOwner(), button = BUTTONS[event.button];
    if (!frame || !button) return;
    const point = cellAt(canvas, frame, event);
    if (!point) return;
    event.preventDefault();
    suppressClick = true;
    if (drag) return;
    if (frame.mouse.tracking === 'press_only') {
      focus();
      void send(report(frame, 'press', point, button, event));
      return;
    }
    drag = {frame, button, pointerId: event.pointerId, point};
    pendingMotion = null;
    recentMotion = null;
    canvas.setPointerCapture(event.pointerId);
    focus();
    void send(report(frame, 'press', point, button, event));
  }, {capture: true});

  canvas.addEventListener('pointermove', event => {
    if (localOverride(event)) resetWheelFraction();
    if (event.defaultPrevented || event.pointerType !== 'mouse' || !event.isPrimary) return;
    const frame = pointerOwner();
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
  canvas.addEventListener('pointerleave', resetWheelFraction);
  globalThis.addEventListener?.('blur', () => { resetWheelFraction(); clearDrag(); });
  for (const type of ['keydown', 'keyup']) {
    globalThis.addEventListener?.(type, event => {
      if (['Alt', 'AltGraph', 'Control', 'Shift', 'Meta'].includes(event.key)) resetWheelFraction();
    }, {capture: true});
  }

  canvas.addEventListener('wheel', event => {
    const frame = owner();
    const geometry = frame && wheelGeometry(frame);
    const mode = event.deltaMode ?? 0;
    if (event.defaultPrevented || localOverride(event) || !Number.isFinite(event.deltaY) ||
        !Number.isFinite(event.deltaX) || !event.deltaY || Math.abs(event.deltaX) > Math.abs(event.deltaY) ||
        ![0, 1, 2].includes(mode) || frame?.mouse.tracking === 'press_only') {
      resetWheelFraction();
      return;
    }
    const point = frame && geometry && cellAt(canvas, frame, event);
    if (!point) { resetWheelFraction(); return; }
    event.preventDefault();
    const alt = Boolean(event.altKey), ctrl = Boolean(event.ctrlKey);
    const direction = Math.sign(event.deltaY);
    if (!wheelFraction || !sameOwner(wheelFraction.frame, frame) ||
        !sameGeometry(wheelFraction.geometry, geometry) || wheelFraction.alt !== alt ||
        wheelFraction.ctrl !== ctrl || wheelFraction.direction !== direction) {
      wheelFraction = {frame, geometry, alt, ctrl, direction, rows: 0};
    }
    let rows = mode === 0 ? event.deltaY / (geometry[3] / frame.rows.length)
      : mode === 1 ? event.deltaY : event.deltaY * frame.rows.length;
    if (!Number.isFinite(rows)) rows = direction * 64;
    wheelFraction.rows += Math.max(-64, Math.min(64, rows));
    const whole = Math.trunc(wheelFraction.rows);
    wheelFraction.rows -= whole;
    if (whole) enqueueWheels(frame, geometry,
      report(frame, whole < 0 ? 'wheel_up' : 'wheel_down', point, null, event), Math.abs(whole));
  }, {passive: false});

  for (const type of ['click', 'auxclick', 'contextmenu']) {
    canvas.addEventListener(type, event => {
      if (suppressClick || (pointerOwner() && !localOverride(event))) event.preventDefault();
      suppressClick = false;
    }, {capture: true});
  }

  return {
    frameChanged() {
      const frame = owner();
      if (drag && !sameOwner(drag.frame, frame) && (frame || !canRelease(drag.frame))) clearDrag(null, false);
      const geometry = (wheelFraction || pendingWheels.length) && frame && wheelGeometry(frame);
      if (wheelFraction && (!sameOwner(wheelFraction.frame, frame) || !sameGeometry(wheelFraction.geometry, geometry))) resetWheelFraction();
      pendingWheels = pendingWheels.filter(next => sameOwner(next.frame, frame) && sameGeometry(next.geometry, geometry));
      if (pendingMotion && !sameOwner(pendingMotion.frame, frame)) pendingMotion = null;
      if (recentMotion && !sameOwner(recentMotion.frame, frame)) recentMotion = null;
      // Accepted input remains a barrier even after its old view retires.
    },
  };
}
