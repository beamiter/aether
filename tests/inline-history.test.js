import test from 'node:test';
import assert from 'node:assert/strict';
import {installInlineHistory} from '../web/inline-history.js';

const settle = () => new Promise(resolve => setImmediate(resolve));
const cell = text => ({text, width: 1, style: 0, foreground: '#ffffff', background: '#000000'});

test('inline paging and wheel share a retained viewport and return to the live frame', async () => {
  const owner = {session: 'terminal', epoch: 1, generation: 1};
  const frame = {session: owner.session, epoch: 1, layout: '1', cols: 1, rows: ['x', 'y', 'z'].map(ch => [cell(ch)])};
  const viewport = {clientHeight: 72, addEventListener(_, listener) { this.wheel = listener; }};
  let painted = [];
  const context = {
    setTransform() {}, clearRect() { painted = []; }, save() {}, restore() {},
    beginPath() {}, rect() {}, clip() {}, fillRect() {},
    fillText(ch) { painted.push(ch); },
  };
  const canvas = {style: {}, addEventListener() {}, getContext: () => context};
  const requests = [];
  const history = 'ABCDEFGHIJ';
  const reader = installInlineHistory({
    canvas, viewport, getOwner: () => owner, getFrame: () => frame, focus() {},
    async fetchHistory(attachment, request) {
      assert.equal(attachment, owner);
      requests.push(request);
      const size = 4;
      const start = request.anchor
        ? Math.min(history.length - size, Math.max(0, Number(request.anchor.row) - 1 + request.shift))
        : history.length - size;
      const characters = [...history.slice(start, start + size)];
      return JSON.stringify({
        owner, version: 5, revision: '1', cols: 1, total_rows: history.length,
        offset: history.length - (start + size), text: characters.join('\n'),
        rows: characters.map((ch, index) => ({
          source: {row: String(start + index + 1), column: 0},
          text_offsets: [index * 2, index * 2 + 1],
          paint: {cells: [cell(ch)], wrapped: false},
        })),
      });
    },
  });
  assert.equal(reader.page({key: 'PageUp'}), false);
  reader.update(owner, true, '1');
  assert.equal(reader.page({key: 'PageUp'}), true);
  await settle();
  assert.deepEqual(painted, ['I', 'J', 'x']);

  const wheel = {deltaX: 0, deltaY: -1, deltaMode: 1, preventDefault() { this.defaultPrevented = true; }};
  viewport.wheel(wheel);
  await settle();
  assert.equal(wheel.defaultPrevented, true);
  assert.equal(requests.length, 2);
  assert.deepEqual(requests[1].anchor, {row: '7', column: 0});
  assert.deepEqual(painted, ['F', 'G', 'H']);

  for (let i = 0; i < 3; i++) {
    assert.equal(reader.page({key: 'PageDown'}), true);
    await settle();
  }
  assert.equal(canvas.hidden, true);
  reader.update(owner, false, '1');
  assert.equal(reader.page({key: 'PageUp'}), false);
});
