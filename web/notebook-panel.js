const MAX_SOURCE = 1024 * 1024;
const POLL_DELAY = 160;
const isRunning = run => run?.outcome.kind === 'running';

export async function readNotebookFile(file) {
  if (!file?.name.toLowerCase().endsWith('.jtnb.md')) throw Error('Select a local .jtnb.md notebook.');
  if (file.size > MAX_SOURCE) throw Error('Notebook exceeds 1 MiB.');
  const bytes = await file.arrayBuffer();
  if (bytes.byteLength > MAX_SOURCE) throw Error('Notebook exceeds 1 MiB.');
  try {
    return new TextDecoder('utf-8', {fatal: true, ignoreBOM: true}).decode(bytes);
  } catch {
    throw Error('Notebook source must be UTF-8.');
  }
}

export function acceptsRun(snapshot, documentId, previous, cell = previous?.cell) {
  return snapshot?.documentId === documentId && snapshot.cell === cell &&
    typeof snapshot.runId === 'string' && Number.isSafeInteger(snapshot.revision) &&
    snapshot.revision >= 0 &&
    (!previous || (snapshot.runId === previous.runId && snapshot.revision > previous.revision));
}

function outcomeLabel(outcome) {
  if (outcome.kind === 'running') return 'Running';
  if (outcome.kind === 'failed') return `Failed (${outcome.reason})`;
  const receipt = outcome.code === null ? `signal ${outcome.signal}` : `exit ${outcome.code}`;
  return `${outcome.kind === 'stopped' ? 'Stopped' : 'Exited'} (${receipt})`;
}

function installNotebook(document, invoke) {
  const byId = id => document.getElementById(id);
  const root = byId('local-notebook');
  const picker = byId('notebook-file');
  const open = byId('notebook-open');
  const close = byId('notebook-close');
  const status = byId('notebook-status');
  const content = byId('notebook-document');
  const parts = byId('notebook-parts');
  const result = byId('notebook-run-result');
  let current = null, run = null, busy = false, stopPending = false, timer = null;
  const request = (action, fields = {}) => invoke('local_notebook', {request: {action, ...fields}});
  const notice = message => { status.textContent = message; };

  function controls() {
    const locked = busy || isRunning(run);
    picker.disabled = locked;
    open.disabled = locked || picker.files.length !== 1;
    close.disabled = locked || !current;
    for (const button of parts.querySelectorAll('button[data-notebook-run]')) button.disabled = locked;
    for (const button of parts.querySelectorAll('button[data-notebook-stop]')) {
      button.disabled = busy || stopPending || !isRunning(run) || Number(button.dataset.notebookStop) !== run.cell;
    }
  }

  function element(tag, value) {
    const node = document.createElement(tag);
    node.textContent = value;
    return node;
  }

  function cellButton(label, action, cell, number, handler) {
    const button = element('button', label);
    button.type = 'button';
    button.dataset[action] = String(cell);
    button.setAttribute('aria-label', `${label} ${number}`);
    button.addEventListener('click', handler);
    return button;
  }

  function showDocument(value, active = null) {
    current = value;
    content.hidden = !value;
    parts.replaceChildren();
    if (value) {
      byId('notebook-title').textContent = value.name;
      byId('notebook-source-id').textContent = `Local snapshot · SHA-256 ${value.sha256}`;
      let cellNumber = 0;
      for (const part of value.parts) {
        if (part.kind === 'text') {
          parts.append(element('pre', part.text));
          continue;
        }
        const number = ++cellNumber;
        const section = document.createElement('section');
        section.dataset.notebookCell = String(part.index);
        const label = part.info || part.interpreter || 'Read-only code';
        section.append(element('h3', `Cell ${number} · ${label}`), element('pre', part.source));
        if (part.interpreter) {
          section.append(
            cellButton('Run cell', 'notebookRun', part.index, number, () => void startCell(part.index)),
            cellButton('Stop cell', 'notebookStop', part.index, number, () => void stopCell(part.index)),
          );
          const interpreter = element('span', part.interpreter);
          interpreter.className = 'note';
          section.append(interpreter);
        }
        parts.append(section);
      }
    }
    showRun(active);
  }

  function showRun(value) {
    clearTimeout(timer);
    run = value;
    result.hidden = !value;
    if (value) {
      const title = parts.querySelector(`[data-notebook-cell="${value.cell}"] h3`);
      byId('notebook-run-title').textContent = `${title?.textContent || 'Cell'} · ${outcomeLabel(value.outcome)}`;
    }
    byId('notebook-stdout').textContent = value?.stdout ?? '';
    byId('notebook-stderr').textContent = value?.stderr ?? '';
    byId('notebook-output-limit').hidden = !value?.truncated;
    if (!isRunning(value)) stopPending = false;
    controls();
    if (isRunning(value)) timer = setTimeout(() => void poll(value), POLL_DELAY);
  }

  async function poll(previous) {
    try {
      const response = await request('poll', {documentId: previous.documentId, runId: previous.runId, after: previous.revision});
      if (run !== previous) return;
      if (response.kind !== 'run') throw Error('Invalid notebook status receipt.');
      if (response.value) {
        if (!acceptsRun(response.value, current?.documentId, previous)) throw Error('Stale notebook status receipt.');
        showRun(response.value);
        return;
      }
    } catch (error) {
      if (run !== previous) return;
      notice(String(error));
    }
    if (run === previous) timer = setTimeout(() => void poll(previous), POLL_DELAY);
  }

  async function perform(action) {
    if (busy) return;
    busy = true;
    controls();
    try { await action(); }
    catch (error) { notice(String(error)); }
    finally { busy = false; controls(); }
  }

  async function startCell(cell) {
    if (!current || isRunning(run)) return;
    await perform(async () => {
      const response = await request('run', {documentId: current.documentId, cell});
      if (response.kind !== 'run' || !acceptsRun(response.value, current.documentId, null, cell)) throw Error('Invalid notebook run receipt.');
      showRun(response.value);
      notice('');
    });
  }

  async function stopCell(cell) {
    if (stopPending || !isRunning(run) || run.cell !== cell) return;
    await perform(async () => {
      const response = await request('stop', {documentId: run.documentId, runId: run.runId});
      if (response.kind !== 'stopping') throw Error('Invalid notebook stop receipt.');
      stopPending = isRunning(run);
    });
  }

  picker.addEventListener('change', controls);
  byId('notebook-open-form').addEventListener('submit', event => {
    event.preventDefault();
    if (picker.files.length !== 1 || isRunning(run)) return;
    void perform(async () => {
      const file = picker.files[0];
      const source = await readNotebookFile(file);
      const response = await request('open', {replacing: current?.documentId ?? null, name: file.name, source});
      if (response.kind !== 'document') throw Error('Invalid notebook open receipt.');
      showDocument(response.value);
      picker.value = '';
      notice('');
    });
  });
  close.addEventListener('click', () => {
    if (!current || isRunning(run)) return;
    void perform(async () => {
      const response = await request('close', {documentId: current.documentId});
      if (response.kind !== 'closed') throw Error('Invalid notebook close receipt.');
      showDocument(null);
      notice('Notebook closed.');
      picker.focus();
    });
  });
  root.hidden = false;
  void perform(async () => {
    const response = await request('inspect');
    if (response.kind !== 'state') throw Error('Invalid notebook state receipt.');
    showDocument(response.value.document, response.value.run);
  });
}

const invoke = globalThis.window?.__TAURI__?.core?.invoke;
if (invoke) installNotebook(document, invoke);
