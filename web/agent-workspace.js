const kindTitles = {cli: 'CLI session', api: 'API chat', task: 'Task'};
const sharedStatus = {
  unavailable: 'Shared workspace is unavailable. Showing local CLI metadata.',
  partial: 'Some shared workspace records are unavailable.',
};

function errorMessage(error) {
  return error?.message ?? String(error);
}

export function installAgentWorkspace(document, {invoke, searchDelay = 180} = {}) {
  const get = id => document.getElementById(id);
  const workspace = get('agent-workspace');
  if (!workspace) return;
  const form = get('workspace-filters');
  const query = get('workspace-query');
  const kind = get('workspace-kind');
  const library = get('workspace-library');
  const project = get('workspace-project');
  const provider = get('workspace-provider');
  const activity = get('workspace-activity');
  const pinned = get('workspace-pinned');
  const status = get('workspace-status');
  const list = get('workspace-rows');
  const refresh = get('workspace-refresh');
  const more = get('workspace-more');
  const history = get('workspace-history');
  const hitList = get('workspace-hits');
  const historyStatus = get('workspace-history-status');
  const historyMore = get('workspace-history-more');
  const detail = get('workspace-detail');
  const detailTitle = get('workspace-detail-title');
  const detailStatus = get('workspace-detail-status');
  const detailFacts = get('workspace-detail-facts');
  const detailText = get('workspace-detail-text');
  const deleteButton = get('workspace-delete');
  const deleteDialog = get('workspace-delete-confirmation');
  const deleteTarget = get('workspace-delete-target');
  const deleteCancel = get('workspace-delete-cancel');
  const deleteConfirm = get('workspace-delete-confirm');
  const deleteMessage = get('workspace-delete-message');

  if (typeof invoke !== 'function') {
    status.textContent = 'Open the Aether desktop app to browse local conversations.';
    for (const control of form.elements) control.disabled = true;
    return;
  }

  let initialized = false;
  let requested = false;
  let refreshOperation = null;
  let generation = 0;
  let evidenceGeneration = 0;
  let filter = null;
  let timer = null;
  let nextOffset = null;
  let nextBefore = null;
  let selectedRow = null;
  let selectedEvidence = null;
  let rowsReady = false;
  let historyReady = false;
  let composing = false;
  let deleteConfirmation = null;
  let deleting = false;
  const rows = new Map();
  const hits = new Map();
  const operations = new Map();

  async function invokeSerial(scope, command, args, current) {
    while (operations.has(scope)) {
      await operations.get(scope).catch(() => {});
      if (!current()) return;
    }
    if (!current()) return;
    const operation = Promise.resolve().then(() => current() ? invoke(command, args) : undefined);
    operations.set(scope, operation);
    try {
      return await operation;
    } finally {
      operations.delete(scope);
    }
  }

  function readFilter() {
    return {
      query: query.value.trim(),
      kind: kind.value,
      library: library.value,
      project: project.value || null,
      provider: provider.value || null,
      activity: activity.value,
      pinned: pinned.checked,
    };
  }

  function element(tag, text) {
    const node = document.createElement(tag);
    node.textContent = text;
    return node;
  }

  function setChoices(select, choices, allLabel) {
    const previous = select.value;
    const options = [allLabel, ...new Set(previous ? [...choices, previous] : choices)].map((label, index) => {
      const option = element('option', label);
      option.value = index === 0 ? '' : label;
      return option;
    });
    select.replaceChildren(...options);
    select.value = previous;
  }

  function updatePressed() {
    for (const [id, entry] of rows) entry.button.setAttribute('aria-pressed', String(id === selectedRow));
    for (const [id, entry] of hits) entry.button.setAttribute('aria-pressed', String(id === selectedEvidence));
  }

  function deletableRow() {
    const row = rows.get(selectedRow)?.row;
    return row?.kind === 'cli' && !row.running && row.can_delete_cli ? row : null;
  }

  function updateDeleteControl() {
    deleteButton.hidden = !deletableRow();
    deleteButton.disabled = !workspace.open || !rowsReady || Boolean(deleteConfirmation) || deleting;
  }

  function dismissDeleteConfirmation({restoreFocus = false} = {}) {
    const operation = deleteConfirmation;
    deleteConfirmation = null;
    if (deleteDialog.open) deleteDialog.close();
    deleteTarget.textContent = '';
    updateDeleteControl();
    if (restoreFocus && operation?.row === deletableRow() && !deleteButton.disabled) {
      deleteButton.focus({preventScroll: true});
    }
  }

  function confirmationIsCurrent(operation) {
    return deleteConfirmation === operation && workspace.open && rowsReady &&
      deletableRow() === operation.row;
  }

  async function prepareDelete() {
    const row = deletableRow();
    if (!row || !workspace.open || !rowsReady || deleting || deleteConfirmation) return;
    const operation = {row, token: null};
    deleteConfirmation = operation;
    deleteMessage.textContent = '';
    updateDeleteControl();
    const current = () => confirmationIsCurrent(operation);
    try {
      const prepared = await invokeSerial('host', 'workspace_prepare_cli_delete', {
        conversation: row.id,
      }, current);
      if (!current()) return;
      if (typeof prepared?.token !== 'string' || !prepared.token || typeof prepared.title !== 'string') {
        throw Error('Invalid saved session confirmation. Refresh and try again.');
      }
      deleteTarget.textContent = prepared.title || 'Untitled CLI session';
      operation.token = prepared.token;
      deleteDialog.showModal();
      deleteCancel.focus();
    } catch (error) {
      if (!current()) return;
      dismissDeleteConfirmation({restoreFocus: true});
      deleteMessage.textContent = errorMessage(error);
    }
  }

  async function confirmDelete() {
    const operation = deleteConfirmation;
    if (!operation?.token || !deleteDialog.open || !confirmationIsCurrent(operation) || deleting) return;
    const {row, token} = operation;
    deleting = true;
    dismissDeleteConfirmation();
    deleteMessage.textContent = 'Deleting saved session…';
    try {
      const result = await invokeSerial('host', 'workspace_delete_cli_session', {token}, () => true);
      if (result?.conversation !== row.id) throw Error('The saved session response could not be verified.');
      if (selectedRow === row.id) {
        const restoreFocus = workspace.open && detail.contains(document.activeElement);
        closeDetail();
        if (restoreFocus) query.focus({preventScroll: true});
      }
      deleteMessage.textContent = 'Saved CLI session deleted.';
    } catch (error) {
      deleteMessage.textContent = errorMessage(error);
    } finally {
      deleting = false;
      request({reload: true});
    }
  }

  function closeDetail({restoreFocus = false} = {}) {
    const button = rows.get(selectedRow)?.button ?? hits.get(selectedEvidence)?.button;
    evidenceGeneration++;
    selectedRow = selectedEvidence = null;
    dismissDeleteConfirmation();
    detail.hidden = true;
    detailStatus.textContent = detailText.textContent = '';
    detailFacts.replaceChildren();
    updatePressed();
    if (restoreFocus) button?.focus({preventScroll: true});
  }

  function showDetail(title, facts, preview, message) {
    detailTitle.textContent = title || 'Untitled conversation';
    detailFacts.replaceChildren(...facts.flatMap(([label, value]) => [element('dt', label), element('dd', value)]));
    detailText.textContent = preview;
    detailText.hidden = !preview;
    detailStatus.textContent = message;
    detail.hidden = false;
    updatePressed();
    updateDeleteControl();
  }

  function showConversation(row) {
    dismissDeleteConfirmation();
    evidenceGeneration++;
    selectedRow = row.id;
    selectedEvidence = null;
    const facts = [
      ['Type', kindTitles[row.kind]],
      ['Provider', row.provider],
      ['Library', row.archived ? 'Archived' : 'Active'],
    ];
    if (row.cwd) facts.push(['Project', row.cwd]);
    if (row.runtime) facts.push(['State', row.runtime]);
    if (row.attention) facts.push(['Needs you', row.attention]);
    if (row.pinned) facts.push(['Pinned', 'Yes']);
    if (row.tags.length) facts.push(['Tags', row.tags.join(', ')]);
    if (row.updated_at_ms > 0) {
      const date = new Date(row.updated_at_ms);
      if (!Number.isNaN(date.valueOf())) facts.push(['Updated', date.toLocaleString()]);
    }
    showDetail(row.title, facts, row.preview, 'Read-only conversation details.');
  }

  async function readEvidence(id, hit) {
    dismissDeleteConfirmation();
    evidenceGeneration++;
    const owner = evidenceGeneration;
    selectedRow = null;
    selectedEvidence = id;
    const title = hit.row.title;
    const facts = [['Provider', hit.row.provider], ['Source', hit.source]];
    showDetail(title, facts, '', 'Loading indexed evidence…');
    try {
      const evidence = await invokeSerial('host', 'workspace_read_document', {
        conversation: hit.row.id,
        eventId: hit.event_id,
        revision: hit.revision,
      }, () => owner === evidenceGeneration);
      if (owner !== evidenceGeneration) return;
      showDetail(title, [['Provider', hit.row.provider], ['Source', evidence.source]], evidence.text,
        evidence.truncated ? 'Read-only evidence excerpt. The stored text is truncated.' : 'Read-only indexed evidence.');
    } catch (error) {
      if (owner === evidenceGeneration) showDetail(title, facts, '', errorMessage(error));
    }
  }

  function makeEntry(onClick) {
    const item = document.createElement('li');
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'workspace-row';
    const title = element('span', '');
    const description = element('small', '');
    button.append(title, description);
    item.append(button);
    button.addEventListener('click', onClick);
    return {item, button, title, description};
  }

  function publishRows(page, append) {
    setChoices(project, page.projects, 'All projects');
    setChoices(provider, page.providers, 'All providers');
    const updated = append ? rows : new Map();
    for (const row of page.rows) {
      let entry = rows.get(row.id);
      if (!entry) entry = makeEntry(() => {
        if (rowsReady && rows.get(row.id) === entry) showConversation(entry.row);
      });
      entry.row = row;
      entry.title.textContent = row.title || 'Untitled conversation';
      entry.description.textContent = [kindTitles[row.kind], row.provider, row.cwd,
        row.running ? 'Running' : '', row.attention ? 'Needs you' : '', row.pinned ? 'Pinned' : ''].filter(Boolean).join(' · ');
      updated.set(row.id, entry);
    }
    const focused = document.activeElement;
    const scroll = list.scrollTop;
    const rendered = [...updated.values()];
    list.replaceChildren(...rendered.map(entry => entry.item));
    list.scrollTop = scroll;
    if (rendered.some(entry => entry.button === focused)) focused.focus({preventScroll: true});
    if (!append) {
      rows.clear();
      for (const [id, entry] of updated) rows.set(id, entry);
    }
    nextOffset = page.next_offset;
    more.hidden = nextOffset == null;
    const count = page.total === 1 ? '1 conversation' : `${page.total} conversations`;
    const shown = rows.size < page.total ? ` ${rows.size} shown.` : '';
    const coverage = sharedStatus[page.shared];
    status.textContent = [count + '.' + shown, coverage,
      page.truncated ? 'The catalog reached its scan limit.' : '', ...(page.warnings ?? [])].filter(Boolean).join(' ');
    if (selectedRow && !rows.has(selectedRow)) closeDetail();
    else if (selectedRow) showConversation(rows.get(selectedRow).row);
    else updatePressed();
  }

  async function loadHistory(ticket, before = null) {
    if (ticket !== generation) return;
    historyMore.disabled = true;
    historyStatus.textContent = 'Searching indexed history…';
    try {
      const page = await invokeSerial('host', 'workspace_search', {filter, before}, () => ticket === generation);
      if (ticket !== generation) return;
      const items = [];
      for (const hit of page.hits) {
        const key = JSON.stringify([hit.row.id, hit.event_id, hit.revision]);
        if (hits.has(key)) continue;
        const entry = makeEntry(() => {
          if (historyReady && hits.get(key) === entry) void readEvidence(key, hit);
        });
        entry.title.textContent = hit.row.title || 'Untitled conversation';
        entry.description.textContent = [hit.source, hit.snippet, hit.truncated ? 'Excerpt' : ''].filter(Boolean).join(' · ');
        hits.set(key, entry);
        items.push(entry.item);
      }
      hitList.append(...items);
      nextBefore = page.next_before;
      historyMore.hidden = nextBefore == null;
      const count = hits.size === 1 ? '1 match shown.' : `${hits.size} matches shown.`;
      historyStatus.textContent = [count, page.coverage].filter(Boolean).join(' ');
      historyReady = true;
      updatePressed();
    } catch (error) {
      if (ticket === generation) {
        historyStatus.textContent = errorMessage(error);
        historyReady = hits.size > 0;
      }
    } finally {
      if (ticket === generation) historyMore.disabled = false;
    }
  }

  async function loadPage(ticket, {reload = false, append = false, offset = 0} = {}) {
    if (ticket !== generation) return;
    const currentFilter = filter;
    const current = () => ticket === generation;
    rowsReady = append;
    more.disabled = refresh.disabled = true;
    status.textContent = append ? 'Loading more conversations…' : 'Loading conversations…';
    try {
      if (refreshOperation) {
        // A superseded refresh can fail on its old query. Retry against the
        // current filters if the catalog has yet to be initialized.
        await refreshOperation.catch(() => {});
        if (!current()) return;
      }
      let page;
      if (reload || !initialized) {
        const operation = invokeSerial('host', 'workspace_refresh', {filter: currentFilter}, current);
        refreshOperation = operation;
        try {
          page = await operation;
          if (page) initialized = true;
        } finally {
          refreshOperation = null;
        }
      } else {
        page = await invokeSerial('catalog', 'workspace_catalog', {filter: currentFilter, offset}, current);
      }
      if (!current()) return;
      publishRows(page, append);
      rowsReady = true;
      updateDeleteControl();
      if (!append) {
        history.hidden = !currentFilter.query;
        if (currentFilter.query) {
          if (page.shared === 'unavailable') {
            historyStatus.textContent = 'Indexed history requires the shared workspace host.';
          } else {
            void loadHistory(ticket);
          }
        }
      }
    } catch (error) {
      if (ticket !== generation) return;
      status.textContent = errorMessage(error);
      if (!append) {
        rows.clear();
        list.replaceChildren();
        closeDetail();
        more.hidden = history.hidden = true;
      }
    } finally {
      if (ticket === generation) more.disabled = refresh.disabled = false;
    }
  }

  function request({reload = false, debounce = false} = {}) {
    requested = true;
    clearTimeout(timer);
    const ticket = ++generation;
    filter = readFilter();
    rowsReady = historyReady = false;
    dismissDeleteConfirmation();
    nextOffset = nextBefore = null;
    more.hidden = history.hidden = historyMore.hidden = true;
    hits.clear();
    hitList.replaceChildren();
    if (selectedEvidence !== null) closeDetail();
    if (debounce) timer = setTimeout(() => loadPage(ticket), searchDelay);
    else void loadPage(ticket, {reload});
  }

  workspace.addEventListener('toggle', () => {
    if (!workspace.open) dismissDeleteConfirmation();
    else if (!requested) request({reload: true});
    else updateDeleteControl();
  });
  refresh.addEventListener('click', () => request({reload: true}));
  form.addEventListener('submit', event => {
    event.preventDefault();
    if (!composing) request();
  });
  query.addEventListener('input', event => {
    if (!composing && !event.isComposing) request({debounce: true});
  });
  query.addEventListener('compositionstart', () => { composing = true; });
  query.addEventListener('compositionend', () => {
    composing = false;
    request({debounce: true});
  });
  query.addEventListener('keydown', event => {
    if (event.key !== 'ArrowDown' || composing || event.isComposing || event.defaultPrevented ||
        event.altKey || event.ctrlKey || event.metaKey || event.shiftKey || !rowsReady) return;
    const first = rows.values().next().value;
    if (first) {
      event.preventDefault();
      first.button.focus();
    }
  });
  list.addEventListener('keydown', event => {
    if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey ||
        event.shiftKey || !rowsReady || !['ArrowUp', 'ArrowDown'].includes(event.key)) return;
    const buttons = [...rows.values()].map(entry => entry.button);
    const index = buttons.indexOf(event.target);
    if (index < 0) return;
    const target = event.key === 'ArrowDown' ? buttons[index + 1] : buttons[index - 1] ?? query;
    if (target) {
      event.preventDefault();
      target.focus();
    }
  });
  for (const control of [kind, library, provider, project, activity, pinned]) {
    control.addEventListener('change', () => request());
  }
  get('workspace-clear').addEventListener('click', () => {
    query.value = project.value = provider.value = '';
    kind.value = activity.value = 'all';
    library.value = 'active';
    pinned.checked = false;
    request();
  });
  more.addEventListener('click', () => {
    if (!more.disabled && nextOffset != null) void loadPage(generation, {append: true, offset: nextOffset});
  });
  historyMore.addEventListener('click', () => {
    if (!historyMore.disabled && nextBefore != null) void loadHistory(generation, nextBefore);
  });
  get('workspace-detail-close').addEventListener('click', () => closeDetail({restoreFocus: true}));
  deleteButton.addEventListener('click', () => void prepareDelete());
  deleteCancel.addEventListener('click', () => dismissDeleteConfirmation({restoreFocus: true}));
  deleteDialog.addEventListener('cancel', event => {
    event.preventDefault();
    dismissDeleteConfirmation({restoreFocus: true});
  });
  deleteConfirm.addEventListener('click', () => void confirmDelete());
  if (workspace.open) request({reload: true});
}

if (typeof document !== 'undefined') {
  installAgentWorkspace(document, {invoke: globalThis.__TAURI__?.core?.invoke});
}
