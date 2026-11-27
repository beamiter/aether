export function installTerminalFocus({document, window, getOwner, isTerminalElement, send}) {
  let attachment = null;
  let focused = false;
  let scheduled = false;
  let windowActive = true;

  function refresh() {
    if (scheduled) return;
    scheduled = true;
    // A move between the canvas and its IME input is one focus transition.
    queueMicrotask(() => {
      scheduled = false;
      const current = getOwner();
      if (current !== attachment) {
        attachment = current;
        focused = false;
      }
      if (!attachment) return;
      const next = windowActive && !document.hidden && document.hasFocus?.() === true &&
        isTerminalElement(document.activeElement);
      if (next !== focused && send(attachment, next)) focused = next;
    });
  }

  document.addEventListener?.('focusin', refresh);
  document.addEventListener?.('focusout', refresh);
  document.addEventListener?.('visibilitychange', refresh);
  window.addEventListener?.('focus', () => {
    windowActive = true;
    refresh();
  });
  window.addEventListener?.('blur', () => {
    windowActive = false;
    refresh();
  });
  return {refresh};
}
