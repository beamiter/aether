import {createFilePreview, selectedReferenceAtCell} from './file-preview.js';
import {nextSourceMatch, sourceDocument, sourceMatchStatus} from './file-preview-source.js';

// Only explicit browser selections grant access. A terminal click supplies a
// candidate for local confirmation, never a file read or a filesystem path.
export function installFilePreviewPanel(document, {getFrame = () => null, getEpoch = () => null} = {}) {
  const get = id => document.getElementById(id);
  const picker = get('preview-folder'), single = get('preview-single');
  const choices = get('preview-files'), showButton = get('preview-show');
  const panel = get('file-preview'), title = get('preview-title'), details = get('preview-details');
  const notice = get('preview-notice'), status = get('preview-panel-status');
  const confirmation = get('preview-confirmation'), confirm = get('preview-confirm-local');
  const tools = get('preview-tools'), sourceTools = get('preview-source-tools');
  const imageTools = get('preview-image-tools'), zoom = get('preview-zoom');
  const viewport = get('preview-content'), body = get('preview-source');
  let raster = get('preview-raster');
  const query = get('preview-query'), matchCase = get('preview-match-case');
  const previous = get('preview-prev'), next = get('preview-next'), matchStatus = get('preview-matches');
  const lineInput = get('preview-line'), lineStatus = get('preview-line-status');
  const reload = get('preview-reload'), copyLine = get('preview-copy-line');
  const fallback = get('preview-copy-fallback'), copyValue = get('preview-copy-value');
  const terminal = document.querySelector('canvas');

  let paths = [], origin = null, reference = null, selectionError = false;
  let snapshotPicker = null, reloadPicker = null, displayed = null;
  let selectedLine = null, lineRequest = null, hits = [], marks = [], active = -1, truncated = false;

  function report(message) {
    if (panel.hidden) notice.textContent = message;
    else {
      notice.textContent = '';
      status.textContent = message;
    }
  }

  function clearDisplay() {
    displayed = null;
    selectedLine = lineRequest = null;
    hits = []; marks = []; active = -1; truncated = false;
    query.value = ''; matchCase.checked = true;
    lineInput.value = ''; lineStatus.textContent = ''; matchStatus.textContent = '';
    delete body.dataset.highlight;
    viewport.scrollTop = viewport.scrollLeft = 0;
    raster.width = raster.height = 1;
    raster.hidden = true;
    zoom.value = 'fit';
    fallback.hidden = true;
    copyValue.value = '';
    tools.hidden = sourceTools.hidden = imageTools.hidden = true;
  }

  function reveal(line, mark) {
    const lineHeight = Number.parseFloat(document.defaultView?.getComputedStyle(body).lineHeight) || 21;
    const top = (line - 1) * lineHeight;
    if (top < viewport.scrollTop || top + lineHeight > viewport.scrollTop + viewport.clientHeight) {
      viewport.scrollTop = Math.max(0, top - lineHeight);
    }
    if (!mark) {
      viewport.scrollLeft = 0;
      return;
    }
    const bounds = viewport.getBoundingClientRect(), hit = mark.getBoundingClientRect();
    if (hit.left < bounds.left || hit.right > bounds.right) {
      viewport.scrollLeft += hit.left - bounds.left - 8;
    }
  }

  function setLine(line, column = null, scroll = false, mark = null) {
    const count = displayed.source.lineCount;
    lineRequest = line === null ? null : {line, column};
    selectedLine = Number.isSafeInteger(line) && line >= 1 && line <= count ? line : null;
    copyLine.disabled = selectedLine === null;
    lineInput.value = line === null ? '' : String(line);
    if (selectedLine === null) {
      delete body.dataset.highlight;
      lineStatus.textContent = line === null ? `${count} source ${count === 1 ? 'line' : 'lines'}`
        : `Line ${line} is outside this file (1–${count}).`;
      return;
    }
    body.dataset.highlight = '';
    body.style.setProperty('--preview-line-offset', selectedLine - 1);
    lineStatus.textContent = `Line ${selectedLine} of ${count}` + (column === null ? '' : ` · reported column ${column}`);
    if (scroll) reveal(selectedLine, mark);
  }

  function updateMatchStatus() {
    previous.disabled = next.disabled = hits.length === 0;
    matchStatus.textContent = query.value ? sourceMatchStatus(hits, active, truncated) : '';
  }

  function renderMatches(retainActive = false) {
    const before = retainActive ? hits[active] : null;
    const {source} = displayed;
    ({hits, truncated} = source.find(query.value, matchCase.checked));
    active = before ? hits.findIndex(hit => hit.line === before.line && hit.start === before.start && hit.end === before.end) : -1;
    marks = [];
    if (!hits.length) {
      body.textContent = source.text;
    } else {
      const content = document.createDocumentFragment();
      let offset = 0;
      hits.forEach((hit, index) => {
        content.append(source.text.slice(offset, hit.start));
        const mark = document.createElement('mark');
        mark.textContent = source.text.slice(hit.start, hit.end);
        mark.classList.toggle('active', index === active);
        content.append(mark);
        marks.push(mark);
        offset = hit.end;
      });
      content.append(source.text.slice(offset));
      body.replaceChildren(content);
    }
    updateMatchStatus();
  }

  function step(previousMatch) {
    if (!displayed?.source || !hits.length) return;
    marks[active]?.classList.remove('active');
    active = nextSourceMatch(hits, active, selectedLine, previousMatch);
    const mark = marks[active];
    mark.classList.add('active');
    setLine(hits[active].line, null, true, mark);
    updateMatchStatus();
  }

  function resizeRaster(resetScroll = false) {
    if (!displayed?.image) return;
    const {width, height} = displayed.image;
    const dpr = document.defaultView?.devicePixelRatio || 1;
    const fit = zoom.value === 'fit';
    const scale = (fit ? 1 : Number(zoom.value) / 100) / dpr;
    raster.classList.toggle('fit', fit);
    raster.style.width = `${Math.max(1 / dpr, width * scale)}px`;
    raster.style.height = fit ? 'auto' : `${Math.max(1 / dpr, height * scale)}px`;
    if (resetScroll) viewport.scrollTop = viewport.scrollLeft = 0;
  }

  const preview = createFilePreview({
    render(item, {retainPreview = false} = {}) {
      if (!item) {
        clearDisplay();
        reloadPicker = null;
        panel.hidden = viewport.hidden = true;
        title.textContent = body.textContent = details.textContent = status.textContent = '';
        return;
      }
      if (origin !== null && origin !== getEpoch()) {
        preview.dismiss();
        report('Terminal session changed. Select a local file explicitly again.');
        return;
      }
      if (!retainPreview) clearDisplay();
      panel.hidden = viewport.hidden = false;
      confirmation.hidden = true;
      title.textContent = item.path;
      if (item.loading) {
        details.textContent = '';
        body.hidden = false;
        body.textContent = item.text;
        if (document.activeElement === confirm || document.activeElement === showButton) body.focus();
        return;
      }

      const top = viewport.scrollTop, left = viewport.scrollLeft;
      const {path, root} = item;
      if (item.bitmap) {
        const target = retainPreview ? raster.cloneNode(false) : raster;
        const context = target.getContext('2d');
        if (!context) throw Error('Image canvas unavailable.');
        target.width = item.width;
        target.height = item.height;
        context.drawImage(item.bitmap, 0, 0);
        const restoreFocus = document.activeElement === body || document.activeElement === raster;
        if (retainPreview) {
          raster.replaceWith(target);
          raster.width = raster.height = 1;
          raster = target;
        }
        displayed = {path, root, image: {width: item.width, height: item.height}};
        raster.hidden = false;
        if (restoreFocus) raster.focus();
        body.hidden = true;
        body.textContent = '';
        details.textContent = `${item.width} × ${item.height} pixels`;
        resizeRaster();
      } else {
        displayed = {path, root, source: sourceDocument(item.text)};
        body.hidden = false;
        lineInput.max = displayed.source.lineCount;
        renderMatches(retainPreview);
        const location = retainPreview ? lineRequest : reference;
        setLine(location?.line ?? null, location?.column ?? null, !retainPreview);
        details.textContent = snapshotPicker === picker ? `Folder snapshot · ${root}` : 'Single-file snapshot';
      }
      tools.hidden = false;
      sourceTools.hidden = !displayed.source;
      imageTools.hidden = !displayed.image;
      fallback.hidden = true;
      copyValue.value = '';
      if (retainPreview) {
        viewport.scrollTop = top;
        viewport.scrollLeft = left;
        report('Local preview reloaded from the new selection.');
      }
    },
    onError(message) {
      selectionError = true;
      report(displayed ? `${message} Showing the previous snapshot.` : message);
    }
  });

  function populate(selected) {
    choices.replaceChildren();
    for (const path of selected) {
      const option = document.createElement('option');
      option.value = option.textContent = path;
      choices.append(option);
    }
    choices.disabled = showButton.disabled = !selected.length;
  }

  async function select(input) {
    const prior = reloadPicker === input ? displayed : null;
    reloadPicker = null;
    if (prior && !input.files?.length) {
      report('Reload canceled. Showing the previous snapshot.');
      return;
    }
    if (prior && origin !== null && origin !== getEpoch()) {
      preview.dismiss();
      report('Terminal session changed. Select a local file explicitly again.');
      return;
    }
    if (!prior) origin = reference = null;
    const other = input === picker ? single : picker;
    other.value = '';
    snapshotPicker = input;
    selectionError = false;
    const options = {retainPreview: !!prior};
    const selected = input === picker ? preview.select(input.files, options)
      : input.files?.length === 1 ? preview.selectFile(input.files[0], options) : (preview.clear(options), []);
    // Single-file mode never gains terminal-relative authority.
    paths = input === picker ? selected : [];
    populate(selected);
    if (prior) {
      if (selectionError) return;
      if (preview.selectedRoot() !== prior.root || !selected.includes(prior.path)) {
        choices.value = '';
        showButton.disabled = true;
        report(`Reselect the snapshot containing ${prior.path} to reload it. Showing the previous snapshot.`);
        return;
      }
      choices.value = prior.path;
      report('Reloading local preview…');
      reload.disabled = true;
      try {
        await preview.show(prior.path, {retainPreview: true});
      } finally {
        reload.disabled = false;
      }
    } else if (!selectionError) {
      report(selected.length ? input === picker ? `${selected.length} local preview files available.`
        : 'One explicit local file selected. Use Preview selected file.' : 'No supported local files selected.');
    }
  }

  picker.addEventListener('change', () => select(picker));
  single.addEventListener('change', () => select(single));
  for (const input of [picker, single]) input.addEventListener('cancel', () => {
    if (reloadPicker !== input) return;
    reloadPicker = null;
    report('Reload canceled. Showing the previous snapshot.');
  });

  reload.addEventListener('click', () => {
    if (!displayed || !snapshotPicker || reload.disabled) return;
    if (origin !== null && origin !== getEpoch()) {
      preview.dismiss();
      report('Terminal session changed. Select a local file explicitly again.');
      return;
    }
    reloadPicker = snapshotPicker;
    reloadPicker.value = '';
    report(reloadPicker === picker ? 'Reselect the trusted folder to reload the preview.' : 'Reselect the local file to reload the preview.');
    reloadPicker.click();
  });

  choices.addEventListener('change', () => {
    origin = reference = null;
    showButton.disabled = !choices.value;
    preview.dismiss();
  });
  async function show() {
    if (!choices.value) return;
    if (origin !== null && origin !== getEpoch()) {
      preview.dismiss();
      report('Terminal session changed. Select a local file explicitly again.');
      return;
    }
    report('');
    await preview.show(choices.value);
  }
  showButton.addEventListener('click', show);
  confirm.addEventListener('click', show);

  get('preview-line-form').addEventListener('submit', event => {
    event.preventDefault();
    if (!displayed?.source) return;
    marks[active]?.classList.remove('active');
    active = -1;
    updateMatchStatus();
    setLine(Number(lineInput.value), null, true);
  });
  query.addEventListener('input', () => { if (displayed?.source) renderMatches(); });
  matchCase.addEventListener('change', () => { if (displayed?.source) renderMatches(); });
  previous.addEventListener('click', () => step(true));
  next.addEventListener('click', () => step(false));
  zoom.addEventListener('change', () => resizeRaster(true));
  document.defaultView?.addEventListener('resize', () => resizeRaster());

  async function copy(label, value) {
    if (value == null) return;
    const owner = displayed;
    fallback.hidden = true;
    copyValue.value = '';
    try {
      await document.defaultView.navigator.clipboard.writeText(value);
      if (owner === displayed) report(`${label} copied.`);
    } catch {
      if (owner !== displayed) return;
      fallback.hidden = false;
      copyValue.value = value;
      copyValue.focus();
      copyValue.select();
      report('Use Ctrl+C or Cmd+C to copy the selected text.');
    }
  }
  get('preview-copy-path').addEventListener('click', () => copy('Path', displayed?.path));
  get('preview-copy-text').addEventListener('click', () => copy('Source', displayed?.source?.text));
  copyLine.addEventListener('click', () => copy('Line', displayed?.source?.line(selectedLine)));

  function closeFromPanel(fromEscape = false) {
    if (panel.hidden) return;
    preview.dismiss();
    report('Preview closed. No terminal input sent.');
    // Held Escape stays away from PTY input. Explicit Close may restore only
    // the same terminal session.
    const target = !fromEscape && origin !== null && origin === getEpoch() ? terminal : showButton;
    (target.disabled ? snapshotPicker : target)?.focus();
  }
  get('preview-close').addEventListener('click', () => closeFromPanel());
  panel.addEventListener('keydown', event => {
    if (panel.hidden || !panel.contains(document.activeElement) || event.key !== 'Escape' ||
        event.defaultPrevented || event.isComposing || event.keyCode === 229 || event.ctrlKey ||
        event.altKey || event.shiftKey || event.metaKey || event.getModifierState?.('AltGraph')) return;
    event.preventDefault();
    event.stopPropagation();
    closeFromPanel(true);
  });
  get('preview-clear').addEventListener('click', () => {
    preview.clear();
    paths = [];
    origin = reference = snapshotPicker = null;
    picker.value = single.value = '';
    populate([]);
    report('Local file access cleared.');
  });

  terminal.addEventListener('click', event => {
    if (event.button !== 0 || !event.ctrlKey || event.altKey || event.metaKey ||
        event.defaultPrevented || event.getModifierState?.('AltGraph') || !paths.length) return;
    const frame = getFrame(), epoch = getEpoch();
    if (epoch === null || !frame || frame.epoch !== epoch || frame.session !== 'pty-demo' ||
        frame.screen === 'alternate') return;
    const applicationMouse = Boolean(frame.mouse && frame.mouse.tracking !== 'off');
    if (Boolean(event.shiftKey) !== applicationMouse) return;
    const rect = event.currentTarget.getBoundingClientRect();
    if (!rect.width || !rect.height) return;
    const column = Math.floor((event.clientX - rect.left) / rect.width * frame.cols);
    const row = Math.floor((event.clientY - rect.top) / rect.height * frame.rows.length);
    // Unsupported names still participate in the exact-name ambiguity check.
    const candidate = selectedReferenceAtCell(frame, row, column, preview.authorizedPaths());
    if (!candidate || !paths.includes(candidate.path)) return;

    event.preventDefault();
    preview.dismiss();
    origin = epoch;
    reference = candidate;
    choices.value = candidate.path;
    panel.hidden = false;
    confirmation.hidden = false;
    title.textContent = candidate.path;
    if (candidate.line !== null) {
      details.textContent = `Requested line ${candidate.line}` +
        (candidate.column === null ? '' : ` · reported column ${candidate.column}`);
    }
    report('Confirm the reference belongs to your selected local folder. No bytes have been read.');
    confirm.focus();
  });
  return preview;
}
