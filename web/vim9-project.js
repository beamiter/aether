export function installProjectTools(document, command) {
  const root = document.getElementById('vim9-project'), el = key => document.getElementById('vproject-' + key);
  const query = el('query'), status = el('status'), choices = el('choices');
  let entry = null, folder = '', activity = null, intent = 0, scanning = null, sending = false;

  function retireMenu() {
    ++intent; clearTimeout(scanning); scanning = null; choices.replaceChildren();
  }
  function controls() {
    el('parent').disabled = !entry || !folder || sending;
    const jobs = activity?.value;
    el('stop-search').disabled = sending || !jobs?.search.running;
    el('stop-build').disabled = sending || !jobs?.build.running;
    el('prev').disabled = el('next').disabled = sending || !jobs?.quickfix.count;
    el('build').querySelector('button').disabled = sending || !!jobs?.build.running;
    el('search').querySelector('button').disabled = sending || !!jobs?.search.running;
  }
  function describe() {
    const data = activity?.value;
    el('root').textContent = data ? [data.root, folder].filter(Boolean).join('/') : '';
    if (!data) {
      el('jobs').textContent = el('output').textContent = '';
      el('diagnostics').replaceChildren();
      controls(); return;
    }
    const {search, build, quickfix, diagnostics} = data;
    const summaries = [];
    if (build.command) {
      const result = build.running ? 'running' : build.cancelled ? 'stopped' : 'exit ' + build.exit_code;
      summaries.push('Build ' + result + (build.truncated ? ' · earlier output omitted' : ''));
    }
    if (search.query) {
      const result = search.running ? 'searching' : search.cancelled ? 'stopped' :
        search.exit_code > 1 || search.exit_code < 0 ? 'exit ' + search.exit_code : search.count + ' matches';
      summaries.push(search.query + ' · ' + result);
      if (search.errors.length) summaries.push(search.errors.join('\n'));
    }
    if (quickfix.count) summaries.push(quickfix.title + ' · ' + quickfix.index + '/' + quickfix.count);
    el('jobs').textContent = summaries.join(' · ');
    el('output').textContent = build.output.join('\n');
    el('diagnostics').replaceChildren(...diagnostics.map(diagnostic => {
      const item = document.createElement('li');
      item.textContent = diagnostic.filename + ':' + diagnostic.line + ':' + diagnostic.column + ' · ' + diagnostic.text;
      return item;
    }));
    if (data.diagnostics_truncated) {
      const note = document.createElement('li'); note.textContent = 'Additional diagnostics omitted.'; el('diagnostics').append(note);
    }
    controls();
  }
  async function send(name, fields, done) {
    if (sending || !entry) return;
    const target = entry;
    sending = true; controls();
    try {
      const response = await command(name, {entryId: target, ...fields});
      if (entry !== target) return;
      if (response.kind !== 'accepted') throw Error('Unexpected Vim command receipt.');
      status.textContent = done;
    } catch (error) { if (entry === target) status.textContent = String(error?.message ?? error); }
    finally { if (entry === target) { sending = false; controls(); } }
  }

  async function lookup(refresh = false) {
    retireMenu();
    if (!entry || !root.open) return;
    const target = entry, ticket = intent, filter = query.value;
    status.textContent = '';
    try {
      const result = await command('lookup', {entryId: entry, folder, query: filter, refresh});
      if (ticket !== intent || entry !== target || query.value !== filter || !root.open) return;
      if (result.kind !== 'choices' || !result.value.id || !Array.isArray(result.value.entries)) throw Error('Invalid Vim project choices.');
      const page = result.value;
      if (page.scanning) {
        status.textContent = 'Reading project files…'; scanning = setTimeout(() => void lookup(), 180); return;
      }
      choices.replaceChildren(...page.entries.map(item => {
        const button = document.createElement('button');
        button.type = 'button'; button.textContent = item.title; button.disabled = !!item.unavailable;
        if (item.unavailable) button.title = item.unavailable;
        button.addEventListener('click', () => {
          if (ticket !== intent || sending || entry !== target) return;
          if (item.directory) { folder = item.key; describe(); void lookup(); return; }
          retireMenu(); void send('choose', {menu: page.id, key: item.key}, 'Sent to Vim.');
        });
        return button;
      }));
      status.textContent = page.truncated ? 'Showing a limited list. Narrow the file or command query.' :
        (page.entries.length ? '' : 'No matching project entries.');
      controls();
    } catch (error) { if (ticket === intent) status.textContent = String(error?.message ?? error); }
  }
  root.addEventListener('toggle', () => { if (root.open) void lookup(); else retireMenu(); });
  query.addEventListener('input', () => { retireMenu(); status.textContent = ''; });
  el('lookup').addEventListener('submit', e => { e.preventDefault(); void lookup(); });
  el('parent').addEventListener('click', () => { folder = folder.split('/').slice(0,-1).join('/'); query.value = ''; describe(); void lookup(); });
  el('refresh').addEventListener('click', () => { folder = query.value = ''; describe(); void lookup(true); });
  el('search').addEventListener('submit', e => { e.preventDefault(); void send('job', {job: {kind:'search', query:el('search-query').value}}, 'Project search requested.'); });
  el('build').addEventListener('submit', e => { e.preventDefault(); void send('job', {job: {kind:'build', command:el('build-command').value}}, 'Vim build requested.'); });
  for (const [key, kind] of [['prev','previous_problem'], ['next','next_problem'], ['stop-search','cancel_search'], ['stop-build','cancel_build']]) {
    el(key).addEventListener('click', () => send('job', {job:{kind}}, 'Sent to Vim.'));
  }
  return {
    revision: () => root.open && entry ? activity?.revision ?? 0 : null,
    update(view) {
      const target = view?.connected && view.ptyLive ? view.entryId : null;
      if (target !== entry) {
        retireMenu(); folder = query.value = ''; sending = false; entry = target; activity = null;
        el('search-query').value = el('build-command').value = status.textContent = '';
        root.hidden = !entry;
        if (!entry) root.open = false;
        describe();
      }
      if (entry && view.activity && (!activity || view.activity.revision > activity.revision)) {
        activity = view.activity; describe();
      }
      if (entry) {
        root.dataset.line = String(view.line);
        root.dataset.byteColumn = String(view.byteColumn);
        root.dataset.windowCount = String(view.windowCount);
      } else { delete root.dataset.line; delete root.dataset.byteColumn; delete root.dataset.windowCount; }
    }
  };
}
