import test from 'node:test';
import assert from 'node:assert/strict';
import {installTerminalSelection} from '../web/terminal-selection.js';

const point = (row, col, extra = {}) => ({
  isPrimary: true,
  pointerType: 'mouse',
  pointerId: 1,
  button: 0,
  clientX: col * 10 + 5,
  clientY: row * 20 + 10,
  preventDefault() {this.defaultPrevented = true;},
  ...extra,
});
const cells = text => [...text].map(text => ({text, width: 1}));
const frame = (revision, overrides = {}) => ({
  session: 'pty-demo',
  epoch: 1,
  revision,
  layout: 1,
  screen: 'primary',
  cols: 4,
  rows: [[{text:'A', width:1}, {text:'界', width:2}, {text:' ', width:0}, {text:'B', width:1}],
    cells('CDEF'), cells('GHIJ')],
  row_wrapped: [true, false, false],
  ...overrides,
});

function harness() {
  const handlers = new Map(), frames = [], availability = [];
  let pointer = null;
  const canvas = {
    addEventListener(name, handler) {handlers.set(name, handler);},
    setPointerCapture(id) {pointer = id;},
    hasPointerCapture(id) {return pointer === id;},
    releasePointerCapture() {pointer = null;},
    getBoundingClientRect() {return {left:0, top:0, width:40, height:60};},
  };
  const overlay = {replaceChildren(...rectangles) {this.rectangles = rectangles;}};
  const selection = installTerminalSelection({
    canvas, overlay, document:{createElement: () => ({style:{}})},
    onFrame: frame => frames.push(frame?.revision),
    onSelection: available => availability.push(available),
  });
  return {
    selection, overlay, frames, availability,
    event(name, event) {handlers.get(name)(event); return event.defaultPrevented;},
    pointer: () => pointer,
  };
}

test('reverse drag holds the painted frame, snaps a wide glyph, and copies hard and soft line boundaries', () => {
  const h = harness();
  h.selection.present(frame(1));
  assert.equal(h.event('pointerdown', point(2, 1)), true);
  h.selection.present(frame(2, {rows:[cells('1234'), cells('5678'), cells('9012')]}));
  h.selection.present(frame(3, {rows:[cells('abcd'), cells('efgh'), cells('ijkl')]}));
  h.event('pointermove', point(0, 2));
  assert.equal(h.selection.visibleFrame().revision, 1);
  assert.deepEqual(h.overlay.rectangles.map(r => r.style), [
    {left:'12px', top:'0px', width:'36px', height:'24px'},
    {left:'0px', top:'24px', width:'48px', height:'24px'},
    {left:'0px', top:'48px', width:'24px', height:'24px'},
  ]);
  h.event('pointerup', point(0, 2));
  assert.equal(h.pointer(), null);
  assert.deepEqual(h.frames, [1, 3]);
  assert.deepEqual(h.selection.snapshot(), {text:'界BCDEF\nGH', revision:1});
  assert.deepEqual(h.overlay.rectangles, []);
  assert.equal(h.event('click', point(0, 2, {ctrlKey:true})), true);
  assert.equal(h.event('click', point(0, 2, {ctrlKey:true})), undefined);
});

test('an unchanged range remains highlighted as the cursor moves, and a new layout cancels an active drag', () => {
  const h = harness();
  h.selection.present(frame(1));
  h.event('pointerdown', point(0, 0));
  h.event('pointerup', point(0, 2));
  h.selection.present(frame(2, {cursor_col:3}));
  assert.equal(h.overlay.rectangles[0].style.width, '36px');
  assert.deepEqual(h.selection.snapshot(), {text:'A界', revision:1});
  h.event('pointerdown', point(0, 0));
  h.event('pointermove', point(0, 1));
  h.selection.present(frame(3, {layout:2}));
  assert.equal(h.pointer(), null);
  assert.equal(h.selection.snapshot(), null);
  assert.equal(h.selection.visibleFrame().revision, 3);
  h.selection.reset();
  assert.equal(h.selection.visibleFrame(), null);
  assert.equal(h.availability.at(-1), false);
});

test('screen transitions and canceled capture cannot complete a gesture or steal Ctrl-click', () => {
  const h = harness();
  h.selection.present(frame(1));
  assert.equal(h.event('pointerdown', point(0, 0, {ctrlKey:true})), undefined);
  assert.equal(h.pointer(), null);
  h.event('pointerdown', point(0, 0));
  h.event('pointermove', point(1, 0, {pointerId:2}));
  assert.deepEqual(h.overlay.rectangles, []);
  h.selection.present(frame(2, {screen:'alternate'}));
  h.event('pointerup', point(1, 0));
  assert.equal(h.selection.snapshot(), null);
  assert.equal(h.selection.visibleFrame().screen, 'alternate');
  h.event('pointerdown', point(1, 1));
  h.event('pointercancel', point(9, 9));
  assert.equal(h.pointer(), null);
  assert.equal(h.selection.snapshot(), null);
});

test('dragging across a wide glyph selects exactly one character and a click stays unclaimed', () => {
  const h = harness();
  h.selection.present(frame(1));
  h.event('pointerdown', point(0, 1));
  h.event('pointerup', point(0, 1));
  assert.equal(h.selection.snapshot(), null);
  assert.equal(h.event('click', point(0, 1)), undefined);
  h.event('pointerdown', point(0, 1));
  h.event('pointerup', point(0, 2));
  assert.deepEqual(h.selection.snapshot(), {text:'界', revision:1});
  assert.equal(h.overlay.rectangles[0].style.width, '24px');
});
