import test from 'node:test';
import assert from 'node:assert/strict';
import {installWorkflows} from '../web/workflows.js';

const turn = () => new Promise(resolve => setImmediate(resolve));
const sameTitle = {catalogId: 'catalog', rows: [{name: 'Local report'}]};

test('an unchanged workflow title is not selectable until the current query rendered', async () => {
  const makeElement = () => ({
    dataset: {}, children: [], listeners: {}, parentElement: {},
    append(...children) { this.children.push(...children); },
    replaceChildren(...children) { this.children = children; },
    addEventListener(name, listener) { this.listeners[name] = listener; },
    setAttribute() {}, focus() {},
  });
  const elements = new Map();
  const element = selector => {
    if (!elements.has(selector)) elements.set(selector, makeElement());
    return elements.get(selector);
  };
  const pending = [];
  let selections = 0;
  const owner = {session: 'local', epoch: 1};
  const workflows = installWorkflows({querySelector: element, createElement: makeElement}, {
    context: () => ({owner, prompt: 'prompt'}),
    retained: () => true,
    invoke: request => {
      if (request.action === 'open') return Promise.resolve(sameTitle);
      if (request.action === 'search') {
        return new Promise((resolve, reject) => pending.push({query: request.query, resolve, reject}));
      }
      assert.equal(request.action, 'select');
      selections++;
      return Promise.resolve({name: 'Local report', command: 'printf report', args: []});
    },
  });
  const results = element('#workflow-results');
  const search = element('#workflow-search');
  const currentButton = () => results.children[0].children[0];
  const enter = value => {
    search.value = value;
    search.listeners.input();
  };

  workflows.toggle();
  await turn();
  assert.equal(results.dataset.searchReady, 'true');

  enter('native-report');
  pending.shift().resolve(sameTitle);
  await turn();
  const intermediate = currentButton();
  assert.equal(intermediate.textContent, 'Local report');
  assert.equal(results.dataset.searchReady, 'true');

  enter('native-report-fixture');
  assert.equal(results.dataset.searchReady, 'false');
  assert.equal(currentButton(), intermediate, 'the old title still matches while search is pending');
  await intermediate.listeners.click();
  assert.equal(selections, 0);
  const current = pending.shift();
  assert.equal(current.query, 'native-report-fixture');
  current.reject(new Error('search interrupted'));
  await turn();
  assert.equal(results.dataset.searchReady, 'false', 'failure cannot promote the old rows');
  await intermediate.listeners.click();
  assert.equal(selections, 0);

  enter('native-report-fixture');
  pending.shift().resolve(sameTitle);
  await turn();
  assert.notEqual(currentButton(), intermediate);
  assert.equal(currentButton().textContent, intermediate.textContent);
  assert.equal(results.dataset.searchReady, 'true');
  await currentButton().listeners.click();
  assert.equal(selections, 1);
  assert.equal(element('#workflow-fields').hidden, false);
});
