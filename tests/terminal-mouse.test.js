import test from 'node:test';
import assert from 'node:assert/strict';
import {installTerminalMouse} from '../web/terminal-mouse.js';

function harness(tracking = 'drag', dispatch = () => {}) {
  const listeners = new Map(), sent = [];
  let frame = {epoch:1, layout:7, mouse:{tracking, generation:8}, cols:6, rows:Array(8),
    session:'pty-demo'};
  let capture = null, epoch = 1, focused = 0;
  const canvas = {
    addEventListener(type, listener, options) {listeners.set(type, {listener, options});},
    getBoundingClientRect() {return {left:10, top:20, width:120, height:160};},
    setPointerCapture(id) {capture = id;},
    hasPointerCapture(id) {return capture === id;},
    releasePointerCapture() {capture = null;},
  };
  const input = installTerminalMouse({
    canvas, getFrame:() => frame, getEpoch:() => epoch,
    send(report) {sent.push(report); return dispatch(report);},
    focus() {focused++;},
  });
  return {
    sent, focused:() => focused, capture:() => capture,
    setFrame(next) {frame = {...frame, ...next}; input.frameChanged();},
    setEpoch(next) {epoch = next; input.frameChanged();},
    event(type, data = {}) {
      const event = {isPrimary:true, pointerType:'mouse', pointerId:1, button:0,
        clientX:49, clientY:61, deltaX:0, deltaY:0, buttons:0,
        preventDefault() {this.defaultPrevented = true;}, ...data};
      listeners.get(type).listener(event);
      return Boolean(event.defaultPrevented);
    },
  };
}

test('application drag maps physical cells and releases the captured button outside the canvas', () => {
  const h = harness();
  assert.equal(h.event('pointerdown', {button:2, ctrlKey:true}), true);
  assert.equal(h.capture(), 1);
  assert.equal(h.focused(), 1);
  h.event('pointermove', {clientX:131, clientY:21, altKey:true});
  h.event('pointerup', {clientX:400, clientY:-4});
  assert.equal(h.event('contextmenu'), true);
  assert.equal(h.capture(), null);
  assert.deepEqual(h.sent.map(({kind, row, col, button, alt, ctrl}) => ({kind, row, col, button, alt, ctrl})), [
    {kind:'press', row:2, col:1, button:'right', alt:false, ctrl:true},
    {kind:'motion', row:0, col:5, button:'right', alt:true, ctrl:false},
    {kind:'release', row:0, col:5, button:'right', alt:false, ctrl:false},
  ]);
  assert.ok(h.sent.every(report => report.layout === 7 && report.generation === 8));
});

test('Shift and non-mouse pointers stay local while vertical wheel pulses obey tracking ownership', () => {
  const h = harness('button');
  assert.equal(h.event('pointerdown', {shiftKey:true}), false);
  assert.equal(h.event('click', {shiftKey:true, ctrlKey:true}), false);
  assert.equal(h.event('pointerdown', {pointerType:'touch'}), false);
  assert.equal(h.event('wheel', {deltaX:20, deltaY:1}), false);
  assert.equal(h.event('wheel', {deltaY:20, shiftKey:true}), false);
  assert.equal(h.event('wheel', {deltaY:-20, ctrlKey:true}), true);
  assert.deepEqual(h.sent, [{layout:7, generation:8, kind:'wheel_up',
    row:2, col:1, button:null, alt:false, ctrl:true}]);
});

test('hover is coalesced and stale pending reports cannot reach a new layout, mode, or epoch', async () => {
  let releaseFirst;
  const first = new Promise(resolve => {releaseFirst = resolve;});
  const h = harness('any', () => first);
  h.event('pointermove', {clientX:40});
  h.event('pointermove', {clientX:60});
  h.event('pointermove', {clientX:100});
  h.setFrame({layout:9});
  releaseFirst();
  await first;
  assert.equal(h.sent.length, 1);
  assert.equal(h.sent[0].layout, 7);
  h.event('pointerdown');
  h.setFrame({mouse:{tracking:'off', generation:10}});
  h.event('pointerup');
  assert.equal(h.capture(), null);
  h.setEpoch(2);
  h.event('pointerdown');
  assert.deepEqual(h.sent.map(report => report.kind), ['motion', 'press']);
});
