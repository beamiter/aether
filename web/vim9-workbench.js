import {acceptChannelFrame, channelCounter} from './frames-v4.js';
import {clearPaint, paint} from './frames.js';
import {installIme} from './ime.js';
import {installPasteReview} from './paste.js';
import {canPaintVim9, paintVim9} from './vim9-canvas.js';
import {installProjectTools} from './vim9-project.js';

const POLL_MS = 96;
const utf8 = new TextEncoder();
const namedKey = /^(Arrow(Up|Down|Left|Right)|Home|End|Insert|Delete|PageUp|PageDown|F([1-9]|1[0-2])|Enter|Escape|Backspace|Tab)$/;

function installVim9(document, invoke) {
  const byId = id => document.getElementById(id);
  const root = byId('vim9-workbench');
  const open = byId('vim9-open');
  const save = byId('vim9-save');
  const quit = byId('vim9-quit');
  const release = byId('vim9-release');
  const fallback = byId('vim9-fallback');
  const viewport = byId('vim9-viewport');
  const stack = byId('vim9-screen');
  const terminalCanvas = byId('vim9-terminal');
  const guiCanvas = byId('vim9-render');
  const input = byId('vim9-input');
  const preedit = byId('vim9-preedit');
  const report = byId('vim9-status');
  let current = null, terminal = null, native = null, timer = null, busy = false;
  let paintedRender = null, paintedScale = null;
  let inputQueue = Promise.resolve(), inputPending = 0;
  const command = (action, fields = {}) => invoke('local_vim9', {request: {action, ...fields}});
  const inputEntry = () => current?.ptyLive ? current.entryId : null;
  const project = installProjectTools(document, async (name, fields) => {
    await inputQueue;
    if (!current?.connected || inputEntry() !== fields.entryId) throw Error('The Vim project changed.');
    return command(name, fields);
  });
  const error = message => { byId('vim9-error').textContent = message; };
  const ime = installIme(input, {
    getEpoch: inputEntry,
    commit: (text, entryId) => {
      if (text.length > 4096 || utf8.encode(text).length > 4096) { error('Vim input exceeds 4096 bytes.'); return; }
      enqueue('text', {text}, entryId);
    },
    preedit: text => { preedit.textContent = text; preedit.hidden = !text; },
    afterEnd: callback => setTimeout(callback, 0),
    canCommit: () => document.activeElement === input,
  });
  const paste = installPasteReview({
    panel: byId('vim9-paste-review'), preview: byId('vim9-paste-preview'), escaped: byId('vim9-paste-escaped'),
    confirm: byId('vim9-paste-confirm'), cancel: byId('vim9-paste-cancel'),
    getEpoch: inputEntry, send: (text, entryId) => enqueue('paste', {text}, entryId), onError: error,
  });

  function enqueue(action, fields, entryId = inputEntry()) {
    if (!entryId || inputEntry() !== entryId) return;
    if (inputPending >= 64) { error('Vim input queue is full.'); return; }
    inputPending++;
    inputQueue = inputQueue.then(async () => {
      if (inputEntry() === entryId) await command(action, {entryId, ...fields});
    }).catch(reason => {
      if (inputEntry() === entryId) error(String(reason));
    }).finally(() => { inputPending--; });
  }

  function controls() {
    open.disabled = busy || !!current;
    save.disabled = quit.disabled = busy || !current?.connected || !current?.ptyLive;
    release.disabled = busy || !current?.drained;
    fallback.disabled = !current;
    input.disabled = !inputEntry();
    viewport.hidden = !current;
  }

  function paintView() {
    const frame = native?.frame;
    const showNative = current?.connected && current.ptyLive && !fallback.checked && canPaintVim9(frame, native?.metrics, terminal);
    if (terminal) paint(terminalCanvas, terminal);
    guiCanvas.hidden = !showNative;
    const scale = globalThis.devicePixelRatio || 1;
    if (showNative && (paintedRender !== frame || paintedScale !== scale)) {
      paintVim9(guiCanvas, frame, native.metrics, scale);
      paintedRender = frame;
      paintedScale = scale;
    }
    stack.dataset.source = showNative ? 'vim9' : current?.ptyLive ? 'terminal' : 'stale';
    const liveLabel = showNative ? 'Vim buffer' : current?.ptyLive ? 'Live terminal' :
      current?.exited ? 'Vim exited · retained terminal image' : 'Vim PTY unavailable · retained terminal image';
    if (current) {
      report.textContent = `${liveLabel}${current.file ? ` · ${current.file}` : ''}${current.mode ? ` · ${current.mode}` : ''}${current.modified ? ' · Modified' : ''}`;
    }
    const cursor = showNative ? frame.cursor : terminal && {row: terminal.cursor_row, column: terminal.cursor_col};
    if (cursor) {
      const metrics = showNative ? native.metrics : {origin_x: 0, origin_y: 0, cell_width: 12, line_height: 24};
      for (const layer of [input, preedit]) {
        layer.style.left = `${metrics.origin_x + cursor.column * metrics.cell_width}px`;
        layer.style.top = `${metrics.origin_y + cursor.row * metrics.line_height}px`;
      }
    }
  }

  function apply(response) {
    if (response.kind === 'idle') {
      clearTimeout(timer);
      current = terminal = native = null;
      project.update(null);
      paintedRender = null;
      fallback.checked = false;
      ime.reset();
      paste.reset();
      clearPaint(terminalCanvas);
      guiCanvas.hidden = true;
      report.textContent = 'Choose a local file to open in Vim9.';
      controls();
      return;
    }
    if (response.kind !== 'state') throw Error('Invalid Vim9 workbench receipt.');
    const view = response.value;
    if (current && (view.entryId !== current.entryId || view.owner.session !== current.owner.session ||
        view.owner.epoch !== current.owner.epoch || view.owner.generation !== current.owner.generation)) {
      throw Error('A Vim9 receipt belonged to a retired entry.');
    }
    if (view.terminal) terminal = acceptChannelFrame(terminal, view.terminal, view.owner, true);
    if (view.render) {
      const generation = channelCounter(view.render.generation);
      if (!native || generation > channelCounter(native.generation)) native = view.render;
    }
    current = view;
    project.update(view);
    if (view.error) error(view.error);
    controls();
    paintView();
    clearTimeout(timer);
    if (!view.drained) timer = setTimeout(() => void poll(view.entryId), POLL_MS);
  }

  async function poll(entryId) {
    try {
      const receipt = await command('poll', {entryId, terminalRevision: terminal?.revision ?? null, renderGeneration: native?.generation ?? null,
        toolsRevision: project.revision()});
      if (current?.entryId === entryId) apply(receipt);
    } catch (reason) {
      if (current?.entryId !== entryId) return;
      error(String(reason));
      timer = setTimeout(() => void poll(entryId), POLL_MS);
    }
  }

  async function action(name, expected = null) {
    if (busy) return;
    busy = true;
    controls();
    error('');
    try {
      if (expected) await inputQueue;
      if (expected && current?.entryId !== expected) return;
      const response = await command(name, expected ? {entryId: expected} : {});
      if (response.kind !== 'accepted') apply(response);
      if (name === 'open' && inputEntry()) input.focus();
    } catch (reason) { error(String(reason)); }
    finally { busy = false; controls(); }
  }

  open.addEventListener('click', () => void action('open'));
  save.addEventListener('click', () => void action('save', current?.entryId));
  quit.addEventListener('click', () => void action('quit', current?.entryId));
  release.addEventListener('click', () => void action('release', current?.entryId));
  fallback.addEventListener('change', paintView);
  terminalCanvas.addEventListener('click', () => { if (inputEntry()) input.focus(); });
  input.addEventListener('keydown', event => {
    if (event.defaultPrevented || !inputEntry() || ime.isComposing() || event.isComposing || event.keyCode === 229 ||
        event.getModifierState?.('AltGraph') || event.metaKey || event.key === 'Dead' || event.key === 'Process' ||
        paste.dismissFromOrigin(event)) return;
    // Ctrl+V is Vim's block-visual key. The desktop paste shortcut is distinct.
    if (event.key === 'Insert' && event.shiftKey && !event.ctrlKey && !event.altKey) return;
    if (!namedKey.test(event.key) && (!((event.ctrlKey || event.altKey) && [...event.key].length === 1))) return;
    event.preventDefault();
    enqueue('key', {name: event.key, shift: event.shiftKey, alt: event.altKey, ctrl: event.ctrlKey});
  });
  input.addEventListener('paste', event => {
    if (!inputEntry()) return;
    event.preventDefault();
    paste.offer(event.clipboardData?.getData('text/plain') || '', input);
  });
  root.hidden = false;
  void action('inspect');
  return async (reference, line, column) => {
    if (busy || current) throw Error('Finish the current Vim9 workbench before opening a preview.');
    busy = true;
    controls();
    try {
      const response = await command('openSnapshot', {reference, line, column});
      apply(response);
      if (!inputEntry()) return false;
      root.open = true;
      input.focus();
      return true;
    } finally { busy = false; controls(); }
  };
}

const invoke = globalThis.window?.__TAURI__?.core?.invoke;
export const openVim9Snapshot = invoke ? installVim9(document, invoke) : null;
