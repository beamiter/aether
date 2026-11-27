import assert from 'node:assert/strict';
import test from 'node:test';
import {installTerminalShell} from '../web/terminal-shell.js';

function palette(actions) {
  let composing = false;
  const doc = {activeElement: null};
  function element(tag) {
    const handlers = new Map();
    const attrs = new Map();
    return {
      tag, children: [], open: false,
      addEventListener(key, handler) { handlers.set(key, handler); },
      send(key, event = {}) {
        event.preventDefault ??= () => { event.defaultPrevented = true; };
        handlers.get(key)?.(event);
        return event;
      },
      setAttribute(key, value) { attrs.set(key, value); },
      getAttribute(key) { return attrs.get(key) ?? null; },
      removeAttribute(key) { attrs.delete(key); },
      append(child) { this.children.push(child); },
      replaceChildren(...children) { this.children = children; },
      querySelectorAll() { return this.children.map(child => child.children[0]); },
      click() { this.send('click'); },
      focus() { doc.activeElement = this; },
      showModal() { this.open = true; },
      close() { this.open = false; },
    };
  }
  const nodes = new Map();
  Object.assign(doc, element('document'), {
    createElement: element,
    querySelector(id) {
      if (!nodes.has(id)) nodes.set(id, element(id));
      return nodes.get(id);
    },
  });
  installTerminalShell(doc, {actions, isComposing: () => composing, keyboardScope: () => true});
  const dialog = doc.querySelector('#command-palette');
  const query = doc.querySelector('#command-palette-query');
  const results = doc.querySelector('#command-palette-results');
  doc.querySelector('#command-palette-open').click();
  return {
    dialog, query, setComposing(value) { composing = value; },
    search(value) { query.value = value; query.send('input'); },
    get buttons() { return results.querySelectorAll(); },
    enter(target = query) { return dialog.send('keydown', {key: 'Enter', target}); },
  };
}

test('palette explains unavailable actions without triggering them', () => {
  const calls = [];
  let writable = false;
  const ui = palette([
    {label: 'Workflows', enabled: () => writable, unavailableReason: () => 'Needs writable jsh',
      run: () => calls.push('workflow')},
    {label: 'Terminal', run: () => calls.push('terminal')},
  ]);
  const [blocked, active] = ui.buttons;
  assert.equal(blocked.getAttribute('aria-disabled'), 'true');
  assert.equal(blocked.children[0].children[0].textContent, 'Needs writable jsh');
  assert.equal(active.getAttribute('aria-disabled'), null);

  ui.search('workflow');
  assert.equal(ui.buttons.length, 1);
  assert.equal(ui.enter().defaultPrevented, true);
  assert.equal(ui.dialog.open, true);
  writable = true;
  ui.buttons[0].click(); // A row shown as disabled stays inert until rendered again.
  assert.deepEqual(calls, []);
  ui.search('workflow');
  ui.enter();
  assert.deepEqual(calls, ['workflow']);
  assert.equal(ui.dialog.open, false);
});

test('query Enter picks the first available action, focused disabled Enter does nothing', () => {
  let called = 0;
  const ui = palette([
    {label: 'Unavailable', enabled: () => false, run: () => assert.fail('ran unavailable action')},
    {label: 'Available', run: () => { called++; }},
  ]);
  ui.buttons[0].focus();
  ui.enter(ui.buttons[0]);
  assert.equal(called, 0);
  assert.equal(ui.dialog.open, true);
  ui.setComposing(true);
  assert.equal(ui.enter().defaultPrevented, undefined);
  assert.equal(called, 0);
  ui.setComposing(false);
  ui.enter();
  assert.equal(called, 1);
});
