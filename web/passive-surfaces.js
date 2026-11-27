import {acceptChannelFrame, channelCounter, CHANNEL_PACKED_VERSION} from './frames-v4.js';

const U32_MAX = 0xffffffff;
const wireId = value => Number.isInteger(value) && value > 0 && value <= U32_MAX;
const sessionId = value => typeof value === 'string' && /^[a-f0-9]{32}$/.test(value);
const stableLeaf = value => typeof value === 'string' && /^[a-z0-9-]{1,48}$/.test(value);

// There is intentionally no input callback. A passive surface only paints.
export function makePassiveSurfaces({paint, clear, allocate}) {
  let nextLayout = 0, slots = [];
  function reconcile(bindings) {
    if (!Array.isArray(bindings) || bindings.length > 2 ||
        bindings.some(x => !x || !stableLeaf(x.leaf) || !wireId(x.epoch) ||
          !sessionId(x.session) || !x.canvas)) throw new Error('invalid passive surface binding');

    if (['leaf','epoch','session','canvas'].some(k =>
      new Set(bindings.map(b => b[k])).size !== bindings.length))
      throw new Error('duplicate passive surface binding');

    let counter = nextLayout;
    const next = bindings.map(binding => {
      const current = slots.find(slot => !binding.invalidate && slot.leaf === binding.leaf &&
        slot.epoch === binding.epoch && slot.session === binding.session && slot.canvas === binding.canvas);
      if (current) return current;
      const layout = allocate ? allocate() : ++counter;
      if (!wireId(layout)) throw new Error('passive surface tokens exhausted');
      const {leaf, epoch, session, canvas} = binding;
      return {leaf, epoch, session, canvas, target:Object.freeze({epoch, layout}), frame:null};
    });
    // Clear retired canvases even if they remain attached as a different
    // token. Late previews must not repaint them under the previous binding.
    for (const old of slots) if (!next.includes(old)) clear(old.canvas);
    slots = next;
    nextLayout = counter;
    return [slots[0]?.target ?? null, slots[1]?.target ?? null];
  }

  function prepare(preview) {
    const current = slots.find(slot => slot.target.layout === preview?.layout);
    if (!current) return null;
    const {canvas, epoch, session, frame:previous} = current;
    const frame = acceptChannelFrame(null, preview.frame,
      {session, epoch, frameVersion:CHANNEL_PACKED_VERSION}, true);
    if (previous && channelCounter(previous.revision) >= channelCounter(frame.revision)) return null;
    return () => {
      if (!slots.includes(current) || current.frame !== previous) return false;
      paint(canvas, frame);
      current.frame = frame;
      return true;
    };
  }

  return {reconcile, prepare};
}
