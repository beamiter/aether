import assert from 'node:assert/strict';
import {test} from 'node:test';
import {paneInputIntent} from '../web/pane-input-intent.js';

function setup() {
  const timers = new Set();
  const intent = paneInputIntent({
    schedule: callback => { timers.add(callback); return callback; },
    unschedule: callback => timers.delete(callback),
  });
  return {intent, timers};
}

test('a split waits for the original input queue including queued releases, then runs once', () => {
  const {intent, timers} = setup();
  const own = {pending: 2};
  const events = [];
  intent.defer(own, () => true, () => events.push('split'), () => events.push('expired'));
  intent.settle({pending: 0});
  own.pending--;
  intent.settle(own);
  assert.deepEqual(events, []);
  own.pending--;
  intent.settle(own);
  intent.settle(own);
  assert.deepEqual(events, ['split']);
  assert.equal(timers.size, 0);
});

test('a retired view or cancelled intent cannot split after its input receipt', () => {
  const {intent, timers} = setup();
  const queue = {pending: 1};
  const unexpected = () => assert.fail('stale action');
  intent.defer(queue, () => false, unexpected, unexpected);
  queue.pending = 0;
  intent.settle(queue);
  assert.equal(timers.size, 0);
  intent.defer(queue, () => true, unexpected, unexpected);
  intent.cancel();
  intent.settle(queue);
  assert.equal(timers.size, 0);
});

test('a newer split replaces the old one, and an expired action never runs late', () => {
  const {intent, timers} = setup();
  const queue = {pending: 1};
  const unexpected = () => assert.fail('stale action');
  let expired = 0;
  intent.defer(queue, () => true, unexpected, unexpected);
  const old = [...timers][0];
  intent.defer(queue, () => true, unexpected, () => expired++);
  old(); // A timer callback already scheduled when replaced cannot clear the new slot.
  assert.equal(timers.size, 1);
  [...timers][0]();
  assert.equal(expired, 1);
  queue.pending = 0;
  intent.settle(queue);
  assert.equal(timers.size, 0);
});
