// Hold at most one explicit split request while its original input drains.
// No input is replayed, and a receipt from a retired attachment cannot run it.
export function paneInputIntent({schedule = setTimeout, unschedule = clearTimeout, maxWait = 1000} = {}) {
  let slot = null;
  function cancel() {
    if (!slot) return;
    unschedule(slot.timer);
    slot = null;
  }
  function defer(queue, stillValid, run, expired) {
    cancel();
    const request = {queue, stillValid, run, timer: null};
    slot = request;
    request.timer = schedule(() => {
      if (slot !== request) return;
      cancel();
      if (stillValid()) expired();
    }, maxWait);
  }
  function settle(queue) {
    const request = slot;
    if (!request || request.queue !== queue || queue.pending !== 0) return;
    cancel();
    if (request.stillValid()) request.run();
  }
  return {defer, settle, cancel};
}
