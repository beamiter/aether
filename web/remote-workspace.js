// The remote catalog has its own reader. Remote identities never enter the local
// Workspace activation, submission, or task controls.
export function installRemoteWorkspace(document, invoke) {
  const el = name => document.getElementById('rws-' + name);
  const select = el('conversation'), query = el('query'), status = el('status');
  let state = null, sequence = 0, intent = 0, pending = false, timer = null, closing = false;
  const sameSource = (a, b) => a && b && a.view === b.view && a.alias === b.alias && a.host_id === b.host_id;
  const matchesSearch = () => state && (select.value || null) === state.conversation && query.value.trim() === state.query;

  async function call(action) {
    const request = ++sequence;
    if (request > 0xffffffff) throw Error('Remote Workspace request counter exhausted.');
    const receipt = await invoke('workspace_remote', {request, action});
    if (receipt?.request !== request || !receipt.value) throw Error('Stale remote Workspace response.');
    return receipt.value;
  }
  function controls() {
    const locked = pending || !state || state.busy;
    for (const key of ['alias','executable','host-id','open']) el(key).disabled = pending || !!state;
    select.disabled = query.disabled = locked || !state.connected;
    el('submit').disabled = locked || !state.connected || (!query.value.trim() && !select.value);
    el('refresh').disabled = locked;
    el('next').disabled = locked || !state.more_rows;
    el('more').disabled = locked || !matchesSearch();
    el('disconnect').disabled = !state || closing;
    for (const b of el('hits').querySelectorAll('button')) b.disabled = locked;
  }
  function showResults() {
    const row = state?.rows.find(item => item.id === select.value);
    el('row').textContent = row ? [row.provider, row.kind, row.directory, row.status].join(' · ') : '';
    const current = matchesSearch();
    el('coverage').textContent = current ? state.coverage ?? '' : '';
    el('hits').replaceChildren();
    el('more').hidden = !current || !state.more_hits;
    const doc = current ? state.document : null;
    el('document').hidden = !doc;
    el('anchor').textContent = doc ? [state.source.alias, doc.source, doc.anchor.event_id, doc.anchor.revision].join(' · ') : '';
    el('text').textContent = doc?.text ?? '';
    if (current) {
      for (const hit of state.hits) {
        const item = document.createElement('li'), button = document.createElement('button');
        const snippet = document.createElement('p');
        const selectedView = state.source.view, revision = state.revision;
        button.type = 'button'; button.textContent = [hit.title, hit.source].join(' · ');
        button.addEventListener('click', () => {
          if (state?.source.view === selectedView && state.revision === revision && matchesSearch()) {
            void command({kind: 'read', anchor: hit.anchor});
          }
        });
        snippet.textContent = hit.snippet;
        item.append(button, snippet); el('hits').append(item);
      }
    }
    controls();
  }
  function render(next) {
    const old = state, previousSelection = select.value;
    state = next;
    el('connect').hidden = !!state; el('browser').hidden = !state;
    select.replaceChildren();
    const all = document.createElement('option');
    all.value = ''; all.textContent = 'All remote conversations'; select.append(all);
    if (state) {
      const source = state.source;
      el('source').textContent = [source.alias, source.host_id, state.connected ? 'Verified read-only catalog' : 'Awaiting host verification'].join(' · ');
      for (const row of state.rows) {
        const option = document.createElement('option'); option.value = row.id;
        option.textContent = row.title || row.id; select.append(option);
      }
      select.value = state.conversation ?? (sameSource(old?.source, source) &&
        state.rows.some(row => row.id === previousSelection) ? previousSelection : '');
      if (!sameSource(old?.source, source)) query.value = state.query;
      status.textContent = state.notice || (state.busy ? 'Reading remote Workspace…' :
        (state.connected ? state.total + ' remote conversations.' : 'Remote Workspace unavailable.'));
    } else {
      el('source').textContent = query.value = '';
      status.textContent = '';
    }
    showResults();
  }
  function schedule() {
    clearTimeout(timer); timer = null;
    if (state?.busy && !closing) {
      timer = setTimeout(() => { void run({operation: 'poll', view: state.source.view}); }, 160);
    }
  }
  async function run(action) {
    const disconnect = action.operation === 'disconnect';
    if (pending && !disconnect) return;
    clearTimeout(timer);
    const ticket = ++intent, previous = state;
    pending = true; closing = disconnect; controls();
    if (disconnect) { el('text').textContent = ''; el('document').hidden = true; }
    try {
      const reply = await call(action);
      if (ticket !== intent) return;
      if (reply.kind === 'closed') {
        render(null);
      } else if (reply.kind === 'state') {
        const next = reply.state;
        if (!next?.source?.view || !Number.isSafeInteger(next.revision) || next.revision < 1 ||
            (previous && (!sameSource(previous.source,next.source) || next.revision <= previous.revision)) ||
            (action.operation === 'connect' && (next.source.alias !== action.alias || next.source.host_id !== action.host_id))) {
          throw Error('Remote Workspace source or revision changed.');
        }
        render(next);
      } else if (action.operation !== 'poll' || reply.kind !== 'unchanged') throw Error('Invalid remote Workspace receipt.');
      schedule();
    } catch (error) {
      if (ticket === intent) status.textContent = String(error?.message ?? error);
    } finally {
      if (ticket === intent) { pending = closing = false; controls(); }
    }
  }
  function command(cmd) {
    if (pending || !state || state.busy) return;
    return run({operation: 'command', view: state.source.view, revision: state.revision, command: cmd});
  }
  el('connect').addEventListener('submit', event => {
    event.preventDefault();
    void run({operation:'connect', alias:el('alias').value.trim(), executable:el('executable').value.trim(), host_id:el('host-id').value.trim()});
  });
  el('disconnect').addEventListener('click', () => { if (state && !closing) void run({operation:'disconnect', view:state.source.view}); });
  el('refresh').addEventListener('click', () => command({kind:'refresh'}));
  el('next').addEventListener('click', () => command({kind:'next_catalog'}));
  el('search').addEventListener('submit', event => { event.preventDefault(); void command({kind:'search', query:query.value.trim(), conversation:select.value || null}); });
  el('more').addEventListener('click', () => command({kind:'next_search'}));
  select.addEventListener('change', showResults);
  query.addEventListener('input', showResults);
  document.getElementById('ssh-workspace').hidden = false;
  void run({operation:'inspect'});
}

const invoke = globalThis.__TAURI__?.core?.invoke;
if (invoke) installRemoteWorkspace(document, invoke);
