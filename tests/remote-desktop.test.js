import test from 'node:test';
import assert from 'node:assert/strict';
import {installRemoteDesktop} from '../web/remote-desktop.js';

class Element {
  value = '';
  checked = false;
  textContent = '';
  children = [];
  events = new Map();

  addEventListener(name, listener) { this.events.set(name, listener); }
  setAttribute() {}
  toggleAttribute() {}
  replaceChildren(...children) { this.children = children; }
  fire(name) { this.events.get(name)({preventDefault() {}}); }
}

const nextTurn = () => new Promise(resolve => setImmediate(resolve));

function fakeRemoteDocument() {
  const elements = new Map();
  const document = {
    getElementById(id) {
      if (!elements.has(id)) elements.set(id, new Element());
      return elements.get(id);
    },
    createElement() { return new Element(); },
  };
  const element = id => document.getElementById(id);
  element('remote-viewer').value = 'x11';
  element('remote-scaling').value = 'auto';
  element('remote-clipboard-mode').value = 'text';
  element('remote-clipboard-direction').value = 'from-host';
  element('remote-clipboard-file-limit').value = '2147483648';
  return {document, element};
}

test('remote tab submits explicit consent and scopes source selection to the published catalog', async t => {
  const {document, element} = fakeRemoteDocument();
  const choice = (id, label) => ({
    selection: {session_id: 'fedcba9876543210fedcba9876543210', source_epoch: '9007199254740993', source_id: id},
    label,
  });
  const first = choice(1, 'Region 1');
  const second = choice(2, '<b>Region 2</b>');
  const switched = {...second, selection: {...second.selection, source_epoch: '9007199254740994'}};
  const idle = {generation: 0, active: false, closing: false, releaseAvailable: false, sources: []};
  let snapshot = idle;
  const calls = [];
  installRemoteDesktop(document, async (name, {action}) => {
    assert.equal(name, 'remote_desktop');
    calls.push(action);
    if (action.type === 'connect') {
      snapshot = {...idle, generation: 1, active: true, currentSource: first,
        sources: [first, second], status: 'Viewing only', embedded: action.options.viewer === 'x11'};
      if (action.options.control) {
        snapshot = {...snapshot, generation: 2, releaseAvailable: true};
      }
    } else if (action.type === 'disconnect') {
      snapshot = {...idle, generation: snapshot.generation};
    } else if (action.type === 'release_input') {
      snapshot = {...snapshot, releaseAvailable: false, status: 'Input released · Viewing only'};
    } else if (action.type === 'select_source' && snapshot.generation === 2) {
      snapshot = {...snapshot, currentSource: switched, sources: [first, switched], requestAvailable: true};
    } else if (action.type === 'request_input') {
      snapshot = {...snapshot, requestAvailable: false};
    }
    return snapshot;
  });
  t.after(() => element('terminal-tab').fire('click'));

  element('remote-tab').fire('click');
  await nextTurn();
  assert.equal(element('terminal-panel').hidden, true);
  assert.equal(element('remote-connect').disabled, false);
  assert.deepEqual(calls[0], {type: 'set_visible', visible: true});
  element('remote-address').value = ' 192.168.0.78:7777 ';
  element('remote-form').fire('submit');
  await nextTurn();
  assert.deepEqual(calls[1], {type: 'connect', generation: 0, options: {
    address: '192.168.0.78:7777', keyFile: '', viewer: 'x11', scaling: 'auto', control: false,
  }});
  assert.equal(element('remote-presentation').textContent, 'Video in this window');
  assert.equal(element('remote-clipboard-status').hidden, true);

  const source = element('remote-source');
  assert.equal(source.children[1].textContent, '<b>Region 2</b>');
  assert.equal(element('remote-release').disabled, true);
  source.value = JSON.stringify(second.selection);
  source.fire('change');
  await nextTurn();
  assert.deepEqual(calls[2], {type: 'select_source', generation: 1, selection: second.selection});

  element('terminal-tab').fire('click');
  await nextTurn();
  assert.equal(element('remote-panel').hidden, true);
  assert.deepEqual(calls.at(-1), {type: 'set_visible', visible: false});
  assert.ok(calls.every(({type}) => type !== 'disconnect'));
  element('remote-tab').fire('click');
  await nextTurn();
  element('remote-disconnect').fire('click');
  await nextTurn();
  assert.deepEqual(calls.at(-1), {type: 'disconnect', generation: 1});

  const control = element('remote-control');
  const viewer = element('remote-viewer');
  control.checked = true;
  viewer.value = 'wayland';
  viewer.fire('change');
  assert.equal(control.checked, false);
  assert.equal(control.disabled, false);
  assert.equal(element('remote-gestures-row').hidden, true);
  control.checked = true;
  control.fire('change');
  assert.equal(element('remote-gestures-row').hidden, false);
  element('remote-gestures').checked = true;
  control.checked = false;
  control.fire('change');
  assert.equal(element('remote-gestures').checked, false);
  control.checked = true;
  control.fire('change');
  element('remote-gestures').checked = true;
  element('remote-form').fire('submit');
  await nextTurn();
  assert.deepEqual(calls.at(-1), {type: 'connect', generation: 1, options: {
    address: '192.168.0.78:7777', keyFile: '', viewer: 'wayland', scaling: 'auto', control: true,
    waylandGestures: true,
  }});
  assert.equal(element('remote-gestures').disabled, true);
  assert.equal(element('remote-presentation').textContent, 'Separate viewer window');
  assert.equal(source.disabled, false);
  assert.equal(element('remote-release').disabled, false);
  element('remote-release').fire('click');
  await nextTurn();
  assert.deepEqual(calls.at(-1), {type: 'release_input', generation: 2});
  assert.equal(element('remote-release').disabled, true);
  source.value = JSON.stringify(second.selection);
  source.fire('change');
  await nextTurn();
  assert.equal(element('remote-release').disabled, false);
  assert.equal(element('remote-release').textContent, 'Request Input');
  assert.ok(calls.every(call => call.type !== 'request_input'));
  element('remote-release').fire('click');
  await nextTurn();
  assert.deepEqual(calls.at(-1), {type: 'request_input', generation: 2, selection: switched.selection});
});

test('automatic reconnect is opt-in for view-only X11 and keeps disconnect available', async t => {
  const {document, element} = fakeRemoteDocument();
  const idle = {generation: 0, active: false, closing: false, releaseAvailable: false, sources: []};
  let snapshot = idle;
  const calls = [];
  installRemoteDesktop(document, async (_name, {action}) => {
    calls.push(action);
    if (action.type === 'connect') {
      snapshot = {...idle, generation: 1, active: true, embedded: true, status: 'Waiting to reconnect…'};
    } else if (action.type === 'disconnect') {
      snapshot = {...idle, generation: 1};
    }
    return snapshot;
  });
  t.after(() => element('terminal-tab').fire('click'));

  element('remote-tab').fire('click');
  await nextTurn();
  const reconnect = element('remote-reconnect');
  const row = element('remote-reconnect-row');
  assert.equal(reconnect.checked, false);
  assert.equal(reconnect.disabled, false);

  for (const id of ['remote-control', 'remote-clipboard']) {
    reconnect.checked = true;
    const conflicting = element(id);
    conflicting.checked = true;
    conflicting.fire('change');
    assert.equal(row.hidden, true);
    assert.equal(reconnect.checked, false);
    conflicting.checked = false;
    conflicting.fire('change');
    assert.equal(row.hidden, false);
  }
  reconnect.checked = true;
  const viewer = element('remote-viewer');
  viewer.value = 'wayland';
  viewer.fire('change');
  assert.equal(row.hidden, true);
  assert.equal(reconnect.checked, false);
  viewer.value = 'x11';
  viewer.fire('change');
  assert.equal(row.hidden, false);

  reconnect.checked = true;
  element('remote-address').value = 'peer';
  element('remote-form').fire('submit');
  await nextTurn();
  assert.deepEqual(calls.at(-1), {type: 'connect', generation: 0, options: {
    address: 'peer', keyFile: '', viewer: 'x11', scaling: 'auto', control: false, reconnectAutomatically: true,
  }});
  assert.equal(element('remote-status').textContent, 'Waiting to reconnect…');
  assert.equal(element('remote-presentation').textContent, 'Video in this window');
  assert.equal(element('remote-source').disabled, true);
  assert.equal(element('remote-release').disabled, true);
  assert.equal(element('remote-disconnect').disabled, false);
  assert.equal(reconnect.disabled, true);

  element('remote-disconnect').fire('click');
  await nextTurn();
  assert.deepEqual(calls.at(-1), {type: 'disconnect', generation: 1});
  assert.equal(reconnect.checked, false);
  assert.equal(reconnect.disabled, false);
});


test('X11 clipboard explicitly requests a per-session policy and reports effective processing', async t => {
  const {document, element} = fakeRemoteDocument();
  const optionsFor = () => calls.filter(action => action.type === 'connect').at(-1)?.options;
  const idle = {generation: 0, active: false, closing: false, releaseAvailable: false, sources: []};
  let snapshot = idle;
  const calls = [];
  installRemoteDesktop(document, async (_name, {action}) => {
    calls.push(action);
    if (action.type === 'connect') {
      snapshot = {...idle, generation: 1, active: true, clipboardRequested: Boolean(action.options.clipboard)};
    } else if (action.type === 'disconnect') {
      snapshot = idle;
    }
    return snapshot;
  });
  t.after(() => element('terminal-tab').fire('click'));

  const change = id => element(id).fire('change');
  const clip = element('remote-clipboard');
  const mode = element('remote-clipboard-mode');
  const direction = element('remote-clipboard-direction');
  const cap = element('remote-clipboard-file-limit');
  const status = element('remote-clipboard-status');
  const transfer = element('remote-clipboard-transfer');
  element('remote-tab').fire('click');
  await nextTurn();
  assert.equal(clip.checked, false);
  assert.equal(element('remote-clipboard-options').hidden, true);
  clip.checked = true;
  change('remote-clipboard');
  assert.equal(element('remote-clipboard-options').hidden, false);
  assert.equal(cap.disabled, true);
  element('remote-address').value = 'peer';
  element('remote-form').fire('submit');
  await nextTurn();
  assert.deepEqual(optionsFor().clipboard, {mode: 'text', direction: 'from-host', fileLimitBytes: '0'});
  assert.equal(status.textContent, 'Clipboard status unavailable');
  assert.equal(transfer.hidden, true);

  element('remote-disconnect').fire('click');
  await nextTurn();
  assert.equal(clip.checked, false);
  clip.checked = true;
  mode.value = 'all';
  direction.value = 'both';
  change('remote-clipboard-mode');
  assert.equal(cap.disabled, false);
  assert.match(element('remote-clipboard-consent').textContent, /current local clipboard/);
  cap.value = '18446744073709551616';
  element('remote-form').fire('submit');
  await nextTurn();
  assert.equal(calls.filter(action => action.type === 'connect').length, 1);
  assert.match(element('remote-feedback').textContent, /file limit/);
  cap.value = '18446744073709551615';
  element('remote-form').fire('submit');
  await nextTurn();
  assert.deepEqual(optionsFor().clipboard, {mode: 'all', direction: 'both', fileLimitBytes: cap.value});

  snapshot = {...snapshot, clipboard: {mode: 'all', direction: 'to-host', file_limit_bytes: cap.value,
    transfer: {direction: 'to-host', kind: 'files', bytes_processed: '9007199254740993', total_bytes: '9007199254741000'}}};
  element('remote-tab').fire('click');
  await nextTurn();
  assert.equal(status.textContent, 'Clipboard · Text, images and files · To host · File limit 18,446,744,073,709,551,615 bytes');
  assert.equal(transfer.textContent, 'Files to host · 9,007,199,254,740,993 of 9,007,199,254,741,000 bytes processed');
  assert.equal(clip.disabled, true);
  assert.equal(element('remote-clipboard-options').hidden, true);

  snapshot = {...snapshot, clipboard: {...snapshot.clipboard, mode: 'text', direction: 'from-host'}};
  element('remote-tab').fire('click');
  await nextTurn();
  assert.equal(status.textContent, 'Clipboard · Text · From host');
  assert.equal(transfer.hidden, true);
  element('remote-disconnect').fire('click');
  await nextTurn();
  assert.equal(status.hidden, true);
  clip.checked = true;
  element('remote-viewer').value = 'wayland';
  change('remote-viewer');
  assert.equal(clip.checked, false);
  assert.equal(element('remote-clipboard-row').hidden, true);
  element('remote-form').fire('submit');
  await nextTurn();
  assert.equal(optionsFor().viewer, 'wayland');
  assert.equal(optionsFor().clipboard, undefined);
});
