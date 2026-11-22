import {test} from 'node:test';
import assert from 'node:assert/strict';
import {canPaintVim9, paintVim9} from '../web/vim9-canvas.js';

const metrics = {origin_x: 0, origin_y: 0, cell_width: 12, line_height: 24};
const terminal = {cols: 10, rows: Array(4)};
const frame = {
  supported: true, rows: 4, columns: 10, revision: 7, completion: null, popups: [], windows: [],
  view: {row: 0, column: 0, rows: 3, columns: 10}, default_style: {background: 0x102030},
  cursor: {visible: true, row: 1, column: 3, width: 2,
    appearance: {shape: 'vertical', percentage: 25, style: {background: 0x22fe88}}},
  runs: [{row: 1, column: 1, text: 'XY', cells: 2, style: {foreground: 0x2054fe}},
         {row: 1, column: 3, text: '你', cells: 2, style: {background: 0x820000}}],
};

test('Vim9 paint only uses native frame geometry supported over the owned terminal', () => {
  assert.equal(canPaintVim9(frame, metrics, terminal), true);
  for (const update of [{supported: false}, {completion: {}}, {popups: [{}]},
    {revision: 0}, {view: {...frame.view, row: 4}}, {columns: 11}]) {
    assert.equal(canPaintVim9({...frame, ...update}, metrics, terminal), false);
  }
  assert.equal(canPaintVim9(frame, {...metrics, cell_width: 0}, terminal), false);
});

test('native render data draws exact runs, clips the editor area and preserves cursor width', () => {
  const calls = [];
  const context = new Proxy({}, {
    get(object, name) {
      return object[name] ?? ((...args) => calls.push([name, ...args]));
    },
  });
  const canvas = {style: {}, getContext: () => context};
  paintVim9(canvas, frame, metrics, 2);
  assert.deepEqual([canvas.width, canvas.height], [240, 192]);
  assert.ok(calls.some(call => call[0] === 'clip'));
  assert.deepEqual(calls.find(call => call[0] === 'rect'), ['rect', 0, 0, 120, 72]);
  assert.deepEqual(calls.filter(call => call[0] === 'fillText'), [
    ['fillText', 'X', 12, 36, 12], ['fillText', 'Y', 24, 36, 12], ['fillText', '你', 36, 36, 24],
  ]);
  assert.deepEqual(calls.filter(call => call[0] === 'fillRect').at(-1), ['fillRect', 36, 24, 3, 24]);
});

test('Vim overlays clip to screen rectangles and stack by z-index then oldest popup', () => {
  const labels = [];
  const context = new Proxy({fillText: value => labels.push(value)}, {
    get: (object, name) => object[name] ?? (() => {}),
  });
  const overlay = (label, id, zindex) => ({
    id, zindex, view: {row: 3, column: 1, rows: 1, columns: 4},
    runs: [{row: 3, column: 2, cells: 1, text: label}],
  });
  const scene = {...frame, completion: overlay('M'), popups: [
    overlay('A', 4, 100), overlay('B', 8, 100), overlay('L', 3, 40),
  ]};
  assert.ok(canPaintVim9(scene, metrics, terminal));
  paintVim9({style: {}, getContext: () => context}, scene, metrics);
  assert.deepEqual(labels.slice(-4), ['L', 'M', 'B', 'A']);
  scene.completion.runs[0].row = 2;
  assert.equal(canPaintVim9(scene, metrics, terminal), false);
});
