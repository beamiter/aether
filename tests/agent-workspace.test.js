import assert from 'node:assert/strict';
import test from 'node:test';
import {setImmediate, setTimeout} from 'node:timers/promises';
import {installAgentWorkspace} from '../web/agent-workspace.js';
import {previewDom} from './support/preview-dom.js';

const defaults = {
  query: '', kind: 'all', library: 'active', project: null, provider: null, activity: 'all', pinned: false,
};

function row(id, fields = {}) {
  return {
    id, kind: 'cli', provider: 'Codex', title: id, cwd: '/local', preview: '', tags: [],
    updated_at_ms: 0, archived: false, pinned: false, runtime: 'idle', running: false,
    attention: null, ...fields,
  };
}

function page(rows = [], fields = {}) {
  return {
    rows, total: rows.length, next_offset: null, projects: ['/local'], providers: ['Codex'],
    shared: 'unavailable', truncated: false, ...fields,
  };
}

function setup(invoke) {
  const {doc, el} = previewDom();
  const base = doc.createElement;
  doc.createElement = tag => {
    const node = base(tag);
    node.attributes = new Map();
    node.setAttribute = (name, value) => node.attributes.set(name, value);
    return node;
  };
  const get = id => el('#workspace-' + id);
  const workspace = el('#agent-workspace');
  workspace.open = false;
  get('kind').value = get('activity').value = 'all';
  get('library').value = 'active';
  get('pinned').checked = false;
  get('detail').hidden = true;
  const dialog = get('delete-confirmation');
  dialog.open = false;
  dialog.showModal = () => { dialog.open = true; };
  dialog.close = () => { dialog.open = false; };
  get('filters').elements = [get('query'), get('refresh')];
  const calls = [];
  installAgentWorkspace(doc, {
    ...(invoke && {invoke(name, args) { calls.push([name, args]); return invoke(name, args); }}),
    searchDelay: 1,
  });
  return {
    doc, get, calls,
    open() { workspace.open = true; workspace.handlers.toggle(); },
    row(index) { return get('rows').children[index].children[0]; },
    hit(index) { return get('hits').children[index].children[0]; },
  };
}

test('expansion admits the catalog and clearing layered filters restores the defaults', async () => {
  const listing = page([row('cli:c1')]);
  const h = setup(() => listing);
  assert.equal(h.calls.length, 0);
  h.open();
  await setImmediate();
  assert.deepEqual(h.calls, [['workspace_refresh', {filter: defaults}]]);
  assert.match(h.get('status').textContent, /1 conversation.*local CLI metadata/);
  assert.equal(h.get('detail').hidden, true);

  h.get('query').value = 'title:release';
  h.get('kind').value = 'task';
  h.get('library').value = 'archived';
  h.get('project').value = '/local';
  h.get('provider').value = 'Codex';
  h.get('activity').value = 'needs_you';
  h.get('pinned').checked = true;
  h.get('activity').handlers.change();
  await setImmediate();
  assert.deepEqual(h.calls[1], ['workspace_catalog', {offset: 0, filter: {
    query: 'title:release', kind: 'task', library: 'archived', project: '/local',
    provider: 'Codex', activity: 'needs_you', pinned: true,
  }}]);
  assert.match(h.get('history-status').textContent, /requires the shared workspace host/);
  h.get('clear').handlers.click();
  await setImmediate();
  assert.deepEqual(h.calls[2], ['workspace_catalog', {offset: 0, filter: defaults}]);
});

test('keyboard browsing keeps rows passive and explicit selection keeps its exact identity after refresh', async () => {
  const initial = page([row('cli:one', {title: '<script>CLI</script>'}), row('api:one', {kind: 'api', title: 'API'})]);
  const reordered = page([row('api:one', {kind: 'api', title: 'Renamed'}), initial.rows[0]]);
  const h = setup(() => h.calls.length === 1 ? initial : reordered);
  h.open();
  await setImmediate();
  const first = h.row(0);
  const second = h.row(1);
  h.get('query').handlers.keydown({key: 'ArrowDown', preventDefault() {}});
  assert.equal(h.doc.activeElement, first);
  h.get('rows').handlers.keydown({key: 'ArrowDown', target: first, preventDefault() {}});
  assert.equal(h.doc.activeElement, second);
  assert.equal(h.get('detail').hidden, true);
  assert.equal(first.children[0].textContent, '<script>CLI</script>');

  second.handlers.click();
  assert.equal(h.get('detail-title').textContent, 'API');
  h.get('refresh').handlers.click();
  await setImmediate();
  assert.equal(h.row(0), second);
  assert.equal(second.attributes.get('aria-pressed'), 'true');
  assert.equal(h.get('detail-title').textContent, 'Renamed');
  assert.equal(h.doc.activeElement, second);
});

test('catalog pagination appends unique conversation identities with the actual offset', async () => {
  const h = setup(name => name === 'workspace_refresh'
    ? page([row('cli:one')], {total: 2, next_offset: 120})
    : page([row('cli:one'), row('api:one', {kind: 'api'})], {total: 2}));
  h.open();
  await setImmediate();
  h.get('more').handlers.click();
  await setImmediate();
  assert.deepEqual(h.calls[1], ['workspace_catalog', {filter: defaults, offset: 120}]);
  assert.equal(h.get('rows').children.length, 2);
  assert.equal(h.get('more').hidden, true);
});

test('history rows request exact source evidence and a later filter discards the old response', async () => {
  const hit = {
    row: row('api:shared', {kind: 'api', title: 'Report'}), source: 'api_message', event_id: 'event-id',
    revision: '7', snippet: '<a>quoted result</a>', truncated: true,
  };
  let respond;
  const h = setup((name, args) => {
    if (name === 'workspace_read_document') {
      return new Promise(resolve => { respond = resolve; });
    }
    if (name === 'workspace_search') {
      return {hits: args.before === null ? [hit] : [], next_before: args.before === null ? '39' : null,
        coverage: 'Existing shared index only.'};
    }
    return page([], {shared: 'partial'});
  });
  h.open();
  await setImmediate();
  h.get('query').value = 'body:report';
  h.get('filters').handlers.submit({preventDefault() {}});
  await setImmediate();
  assert.match(h.get('history-status').textContent, /1 match shown.*shared index/);
  assert.match(h.hit(0).children[1].textContent, /<a>quoted result<\/a>/);
  h.get('history-more').handlers.click();
  await setImmediate();
  assert.equal(h.calls.at(-1)[1].before, '39');
  assert.equal(h.get('history-more').hidden, true);

  const originalHit = h.hit(0);
  originalHit.handlers.click();
  await setImmediate();
  assert.deepEqual(h.calls.at(-1), ['workspace_read_document', {
    conversation: 'api:shared', eventId: 'event-id', revision: '7',
  }]);
  h.get('clear').handlers.click();
  await setImmediate();
  respond({source: 'api_message', text: 'Stale secret', truncated: false});
  await setImmediate();
  originalHit.handlers.click();
  assert.equal(h.get('detail').hidden, true);
  assert.equal(h.get('detail-text').textContent, '');
});

test('only the newest catalog response is rendered and composing input waits for completion', async () => {
  let first;
  const h = setup((name, args) => {
    if (name === 'workspace_refresh') return page();
    if (args.filter.query === 'older') return new Promise(resolve => { first = resolve; });
    return page([row('cli:new', {title: args.filter.query})]);
  });
  h.open();
  await setImmediate();
  h.get('query').value = 'older';
  h.get('filters').handlers.submit({preventDefault() {}});
  await setImmediate();
  h.get('query').handlers.compositionstart();
  h.get('query').value = '最新';
  h.get('query').handlers.input({isComposing: true});
  h.get('filters').handlers.submit({preventDefault() {}});
  assert.equal(h.calls.length, 2);
  h.get('query').handlers.compositionend();
  await setTimeout(20);
  assert.equal(h.calls.length, 2);
  first(page([row('cli:old', {title: 'Older response'})]));
  await setImmediate();
  assert.equal(h.calls.length, 3);
  assert.equal(h.row(0).children[0].textContent, '最新');
  assert.equal(h.get('detail').hidden, true);
});

test('a busy history request releases only the newest queued search', async () => {
  let finish;
  const h = setup((name, args) => {
    if (name !== 'workspace_search') return page([], {shared: 'available'});
    if (args.filter.query === 'first') return new Promise(resolve => { finish = resolve; });
    return {hits: [], next_before: null, coverage: args.filter.query};
  });
  h.open();
  await setImmediate();
  for (const query of ['first', 'intermediate', 'latest']) {
    h.get('query').value = query;
    h.get('filters').handlers.submit({preventDefault() {}});
    await setImmediate();
  }
  const searched = () => h.calls.filter(([name]) => name === 'workspace_search').map(([, args]) => args.filter.query);
  assert.deepEqual(searched(), ['first']);
  finish({hits: [], next_before: null, coverage: 'Old coverage'});
  await setImmediate();
  assert.deepEqual(searched(), ['first', 'latest']);
  assert.match(h.get('history-status').textContent, /latest/);
});

test('a failed initial search can be corrected while refresh is in progress', async () => {
  let fail;
  const h = setup(() => h.calls.length === 1
    ? new Promise((_resolve, reject) => { fail = reject; })
    : page([row('cli:one')]));
  h.open();
  await setImmediate();
  h.get('query').value = 'title:valid';
  h.get('filters').handlers.submit({preventDefault() {}});
  fail('Invalid original filter');
  await setImmediate();
  assert.deepEqual(h.calls.at(-1), ['workspace_refresh', {filter: {...defaults, query: 'title:valid'}}]);
  assert.match(h.get('status').textContent, /1 conversation/);
});

test('only a stopped saved CLI session can be confirmed and deleted', async () => {
  const saved = row('cli:saved', {can_delete_cli: true});
  const running = row('cli:running', {running: true, can_delete_cli: true});
  const api = row('api:chat', {kind: 'api', can_delete_cli: true});
  let removed = false;
  const h = setup(name => {
    if (name === 'workspace_prepare_cli_delete') return {title: '<saved>', token: 'exact-token'};
    if (name === 'workspace_delete_cli_session') {
      removed = true;
      return {conversation: saved.id};
    }
    return page(removed ? [api, running] : [api, running, saved]);
  });
  h.open();
  await setImmediate();
  for (const index of [0, 1]) {
    h.row(index).handlers.click();
    assert.equal(h.get('delete').hidden, true);
    h.get('delete').handlers.click();
  }
  assert.equal(h.calls.length, 1);
  h.get('library').value = 'all';
  h.get('library').handlers.change();
  await setImmediate();
  h.row(2).handlers.click();
  assert.equal(h.get('delete').hidden, false);
  h.get('delete').handlers.click();
  await setImmediate();
  assert.equal(h.get('delete-confirmation').open, true);
  assert.equal(h.get('delete-target').textContent, '<saved>');
  assert.equal(h.doc.activeElement, h.get('delete-cancel'));
  assert.deepEqual(h.calls.at(-1), ['workspace_prepare_cli_delete', {conversation: saved.id}]);
  h.get('delete-confirmation').handlers.cancel({preventDefault() {}});
  h.get('delete-confirm').handlers.click();
  assert.equal(h.get('delete-confirmation').open, false);
  assert.equal(h.doc.activeElement, h.get('delete'));
  assert.equal(removed, false);

  h.get('delete').handlers.click();
  await setImmediate();
  h.get('delete-confirm').handlers.click();
  h.get('delete-confirm').handlers.click();
  await setImmediate();
  assert.deepEqual(h.calls.at(-2), ['workspace_delete_cli_session', {token: 'exact-token'}]);
  assert.deepEqual(h.calls.at(-1), ['workspace_refresh', {filter: {...defaults, library: 'all'}}]);
  assert.equal(h.get('delete-message').textContent, 'Saved CLI session deleted.');
  assert.equal(h.get('detail').hidden, true);
  assert.equal(h.get('rows').children.length, 2);
});

test('selection and filter changes discard stale deletion confirmations', async () => {
  const first = row('cli:one', {can_delete_cli: true});
  const second = row('cli:two', {can_delete_cli: true});
  let finish;
  const h = setup((name, args) => {
    if (name === 'workspace_prepare_cli_delete') {
      return args.conversation === first.id
        ? new Promise(resolve => { finish = resolve; }) : {title: second.title, token: 'second-token'};
    }
    return page([first, second]);
  });
  h.open();
  await setImmediate();
  h.row(0).handlers.click();
  h.get('delete').handlers.click();
  await setImmediate();
  h.row(1).handlers.click();
  finish({title: first.title, token: 'stale-token'});
  await setImmediate();
  assert.equal(h.get('delete-confirmation').open, false);
  assert.equal(h.get('detail-title').textContent, second.title);

  h.get('delete').handlers.click();
  await setImmediate();
  assert.equal(h.get('delete-confirmation').open, true);
  h.get('activity').value = 'running';
  h.get('activity').handlers.change();
  await setImmediate();
  h.get('delete-confirm').handlers.click();
  assert.equal(h.get('delete-confirmation').open, false);
  assert.equal(h.calls.some(([name]) => name === 'workspace_delete_cli_session'), false);
});

test('an acknowledged deletion refreshes the latest filters and retains another selection', async () => {
  const first = row('cli:one', {can_delete_cli: true});
  const second = row('cli:two', {can_delete_cli: true});
  let finish;
  let removed = false;
  const h = setup(name => {
    if (name === 'workspace_prepare_cli_delete') return {title: first.title, token: 'first-token'};
    if (name === 'workspace_delete_cli_session') {
      return new Promise(resolve => { finish = resolve; });
    }
    return page(removed ? [second] : [first, second]);
  });
  h.open();
  await setImmediate();
  h.row(0).handlers.click();
  h.get('delete').handlers.click();
  await setImmediate();
  h.get('delete-confirm').handlers.click();
  await setImmediate();
  h.get('pinned').checked = true;
  h.get('pinned').handlers.change();
  await setImmediate();
  h.row(1).handlers.click();
  assert.equal(h.get('delete').disabled, true);
  removed = true;
  finish({conversation: first.id});
  await setImmediate();
  assert.deepEqual(h.calls.at(-1), ['workspace_refresh', {filter: {...defaults, pinned: true}}]);
  assert.equal(h.get('detail').hidden, false);
  assert.equal(h.get('detail-title').textContent, second.title);
  assert.equal(h.get('delete').disabled, false);
});

test('browser-only mode keeps the workspace controls inactive', () => {
  const h = setup();
  assert.match(h.get('status').textContent, /desktop app/);
  assert.equal(h.get('query').disabled, true);
});
